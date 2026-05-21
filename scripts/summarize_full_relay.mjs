#!/usr/bin/env node

import fs from "node:fs";

const logPaths = process.argv.slice(2);
const useDefaultLogPaths = logPaths.length === 0;

function existingLog(path) {
    return path && fs.existsSync(path) ? path : null;
}

const cpuPath = existingLog(useDefaultLogPaths ? "logs/meta-cpu.log" : logPaths[0]);
const memoryPath = existingLog(useDefaultLogPaths ? "logs/meta-memory.log" : logPaths[1]);

function read(path) {
    return fs.readFileSync(path, "utf8");
}

function configuredFanout() {
    const value = Number(process.env.FANOUT_USERS ?? "30");
    if (Number.isInteger(value) && value > 0) {
        return value;
    }
    return 30;
}

function parseScenario(name) {
    const match = name.match(/^(?:audio|video)-(\d+)B-(\d+)dst$/);
    if (!match) {
        throw new Error(`unexpected scenario name: ${name}`);
    }
    return {
        key: `${match[1]}-${match[2]}`,
        payload: Number(match[1]),
        users: Number(match[2]),
    };
}

function parseCpu(path, variant) {
    const lines = read(path).split(/\r?\n/);
    const rows = new Map();

    for (let index = 0; index < lines.length; index += 1) {
        const scenario = lines[index].match(new RegExp(`^full_relay_rtp/${variant}/(.+)$`));
        if (!scenario) {
            continue;
        }

        const parsed = parseScenario(scenario[1]);
        for (let offset = index + 1; offset < Math.min(lines.length, index + 8); offset += 1) {
            const timing = lines[offset].match(
                /time:\s+\[([0-9.]+)\s+([^\s]+)\s+([0-9.]+)\s+([^\s]+)\s+([0-9.]+)\s+([^\s]+)\]/,
            );
            if (!timing) {
                continue;
            }

            rows.set(parsed.key, {
                ...parsed,
                low: Number(timing[1]),
                mid: Number(timing[3]),
                high: Number(timing[5]),
                unit: timing[4],
            });
            break;
        }
    }

    return rows;
}

function timingMicros(row) {
    if (row.unit === "ns") {
        return row.mid / 1000;
    }
    if (row.unit === "µs" || row.unit === "us") {
        return row.mid;
    }
    if (row.unit === "ms") {
        return row.mid * 1000;
    }
    throw new Error(`unexpected timing unit: ${row.unit}`);
}

function formatTiming(row) {
    const micros = timingMicros(row);
    if (micros >= 1000) {
        return `${(micros / 1000).toFixed(2)} ms`;
    }
    return `${Math.round(micros)} us`;
}

function formatPercent(before, after, metric) {
    const value = (1 - after / before) * 100;
    const rounded = Math.round(value);
    if (metric === "gain" && rounded === 0) {
        return "~0%";
    }
    return `${rounded < 0 ? "~-" : "~"}${Math.abs(rounded)}%`;
}

function orderedRows(rows) {
    return [...rows.values()].sort((left, right) => left.payload - right.payload || left.users - right.users);
}

function cpuTable(baseRows, arcRows) {
    const lines = [
        "### str0m direct RTP relay path CPU",
        "",
        "| payload | users | Default Meta Vec | Ref Meta | gain |",
        "|---|---:|---:|---:|---:|",
    ];

    for (const base of orderedRows(baseRows)) {
        const arc = arcRows.get(base.key);
        if (!arc) {
            throw new Error(`missing Ref Meta CPU row for ${base.key}`);
        }

        lines.push(
            `| ${base.payload} B | ${base.users} | ${formatTiming(base)} | ${formatTiming(arc)} | ${formatPercent(
                timingMicros(base),
                timingMicros(arc),
                "gain",
            )} |`,
        );
    }

    return lines.join("\n");
}

function parseMemory(path, variant) {
    const rows = new Map();

    for (const line of read(path).split(/\r?\n/)) {
        if (!line.startsWith("| full_relay_rtp |")) {
            continue;
        }

        const columns = line
            .split("|")
            .map((column) => column.trim())
            .slice(1, -1);
        const [, scenario, rowVariant, packets, allocCalls, , , allocatedBytes] = columns;

        if (rowVariant !== variant) {
            continue;
        }

        const parsed = parseScenario(scenario);
        rows.set(parsed.key, {
            ...parsed,
            packets: Number(packets),
            allocCalls: Number(allocCalls),
            allocatedBytes: Number(allocatedBytes),
        });
    }

    return rows;
}

function formatBytes(bytes) {
    if (bytes >= 1024 * 1024) {
        return `${(bytes / 1024 / 1024).toFixed(2)} MiB`;
    }
    if (bytes >= 1024) {
        return `${Math.round(bytes / 1024)} KiB`;
    }
    return `${bytes} B`;
}

function memoryTable(baseRows, arcRows) {
    const lines = [
        "### str0m direct RTP relay path allocations",
        "",
        "| payload | users | Default Meta Vec | Ref Meta | saved |",
        "|---|---:|---:|---:|---:|",
    ];

    for (const base of orderedRows(baseRows)) {
        const arc = arcRows.get(base.key);
        if (!arc) {
            throw new Error(`missing Ref Meta allocation row for ${base.key}`);
        }

        lines.push(
            `| ${base.payload} B | ${base.users} | ${formatBytes(base.allocatedBytes)} / ${
                base.allocCalls
            } allocs | ${formatBytes(arc.allocatedBytes)} / ${arc.allocCalls} allocs | ${formatPercent(
                base.allocatedBytes,
                arc.allocatedBytes,
                "saved",
            )} |`,
        );
    }

    return lines.join("\n");
}

function parseCallgrindId(id) {
    let match = id.match(/^p(\d+)b_(\d+)users?$/);
    if (match) {
        return {
            key: `${match[1]}-${match[2]}`,
            payload: Number(match[1]),
            users: Number(match[2]),
        };
    }

    match = id.match(/^p(\d+)b_configured_users$/);
    if (!match) {
        throw new Error(`unexpected callgrind id: ${id}`);
    }
    return {
        key: `${match[1]}-${configuredFanout()}`,
        payload: Number(match[1]),
        users: configuredFanout(),
    };
}

function callgrindGroup(functionName) {
    const groups = {
        rtp_event_fanout_base_vec: ["rtp_event_fanout", "RTP event fanout", 0, "base"],
        rtp_event_fanout_arc_meta: ["rtp_event_fanout", "RTP event fanout", 0, "arc"],
        full_relay_base_vec: ["full_relay", "full relay", 1, "base"],
        full_relay_arc_meta: ["full_relay", "full relay", 1, "arc"],
    };
    const group = groups[functionName];
    if (!group) {
        return null;
    }
    return {
        key: group[0],
        label: group[1],
        order: group[2],
        variant: group[3],
    };
}

function walkFiles(dir) {
    const files = [];
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const path = `${dir}/${entry.name}`;
        if (entry.isDirectory()) {
            files.push(...walkFiles(path));
        } else {
            files.push(path);
        }
    }
    return files;
}

function metricValue(metric) {
    if (typeof metric === "number") {
        return metric;
    }
    if (metric && typeof metric === "object" && "Int" in metric) {
        return Number(metric.Int);
    }
    if (metric && typeof metric === "object" && "Float" in metric) {
        return Number(metric.Float);
    }
    throw new Error(`unexpected callgrind metric value: ${JSON.stringify(metric)}`);
}

function singleMetricValue(metricDiff) {
    if (!metricDiff) {
        return null;
    }

    const metrics = metricDiff.metrics;
    if (metrics && typeof metrics === "object") {
        for (const key of ["Single", "This", "New", "Left", "Right"]) {
            if (key in metrics) {
                return metricValue(metrics[key]);
            }
        }

        const values = Object.values(metrics).filter(
            (value) =>
                typeof value === "number" ||
                (value && typeof value === "object" && ("Int" in value || "Float" in value)),
        );
        if (values.length === 1) {
            return metricValue(values[0]);
        }
    }

    return metricValue(metricDiff);
}

function callgrindSummary(summary) {
    const profiles = Array.isArray(summary.profiles) ? summary.profiles : [];
    for (const profile of profiles) {
        const toolSummary = profile?.summaries?.total?.summary;
        if (toolSummary?.Callgrind) {
            return toolSummary.Callgrind;
        }
    }
    return null;
}

function parseCallgrindSummaries(dir) {
    if (!dir || !fs.existsSync(dir)) {
        return [];
    }

    const rows = [];
    for (const path of walkFiles(dir)) {
        if (!path.endsWith("/summary.json")) {
            continue;
        }

        const summary = JSON.parse(read(path));
        const group = callgrindGroup(summary.function_name);
        if (!group || !summary.id) {
            continue;
        }

        const instructions = singleMetricValue(callgrindSummary(summary)?.Ir);
        if (instructions === null) {
            continue;
        }

        const scenario = parseCallgrindId(summary.id);
        rows.push({
            ...scenario,
            group: group.label,
            groupKey: group.key,
            groupOrder: group.order,
            variant: group.variant,
            instructions,
        });
    }

    const byKey = new Map();
    for (const row of rows) {
        const key = `${row.groupKey}-${row.key}`;
        const combined = byKey.get(key) ?? {
            payload: row.payload,
            users: row.users,
            group: row.group,
            groupKey: row.groupKey,
            groupOrder: row.groupOrder,
        };
        combined[row.variant] = row.instructions;
        byKey.set(key, combined);
    }

    return [...byKey.values()]
        .filter((row) => row.base !== undefined && row.arc !== undefined)
        .sort(
            (left, right) =>
                left.groupOrder - right.groupOrder || left.payload - right.payload || left.users - right.users,
        );
}

function formatInstructionCount(value) {
    if (value >= 1_000_000_000) {
        return `~${(value / 1_000_000_000).toFixed(2)}B`;
    }
    if (value >= 1_000_000) {
        return `~${(value / 1_000_000).toFixed(2)}M`;
    }
    if (value >= 1_000) {
        return `~${(value / 1_000).toFixed(1)}K`;
    }
    return `${value}`;
}

function callgrindTable(rows) {
    const lines = [
        "### str0m direct RTP relay path Callgrind instructions",
        "",
        "| path | payload | users | Default Meta Vec | Ref Meta | gain |",
        "|---|---:|---:|---:|---:|---:|",
    ];

    for (const row of rows) {
        lines.push(
            `| ${row.group} | ${row.payload} B | ${row.users} | ${formatInstructionCount(
                row.base,
            )} | ${formatInstructionCount(row.arc)} | ${formatPercent(row.base, row.arc, "gain")} |`,
        );
    }

    return lines.join("\n");
}

const callgrindRows = parseCallgrindSummaries(process.env.CALLGRIND_DIR);
const hasCpu = Boolean(cpuPath);
const hasMemory = Boolean(memoryPath);
const criterionRequired = process.env.CRITERION_REQUIRED === "true";
if (process.env.CALLGRIND_REQUIRED === "true" && callgrindRows.length === 0) {
    throw new Error(`no Callgrind summaries found in ${process.env.CALLGRIND_DIR ?? "<unset>"}`);
}
if (criterionRequired && !hasCpu) {
    throw new Error("Criterion results are required but CPU logs are missing");
}
if (!hasCpu && !hasMemory && callgrindRows.length === 0) {
    throw new Error("no benchmark results found");
}

const criterionSettings =
    process.env.RUN_CRITERION === "false"
        ? "Criterion disabled"
        : `${process.env.FULL_RELAY_ROUNDS ?? "default"} inbound packets per run, sample size ${
              process.env.SAMPLE_SIZE ?? "criterion default"
          }, measurement time ${
              process.env.MEASUREMENT_TIME ? `${process.env.MEASUREMENT_TIME}s` : "criterion default"
          }`;
const sections = [
    `settings: allocator ${process.env.ALLOCATOR ?? "system"}, fanout users ${configuredFanout()}, ${criterionSettings}, ${
        process.env.FULL_RELAY_CALLGRIND_ROUNDS ?? "default"
    } inbound packets per Callgrind run`,
];

if (hasCpu) {
    sections.push("", cpuTable(parseCpu(cpuPath, "base_vec"), parseCpu(cpuPath, "arc_meta")));
}

if (hasMemory) {
    sections.push("", memoryTable(parseMemory(memoryPath, "base_vec"), parseMemory(memoryPath, "arc_meta")));
}

if (callgrindRows.length > 0) {
    sections.push("", callgrindTable(callgrindRows));
}

console.log(sections.join("\n"));
