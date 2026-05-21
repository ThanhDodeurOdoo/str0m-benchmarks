#!/usr/bin/env node

import fs from "node:fs";

const logPaths = process.argv.slice(2);
const useDefaultLogPaths = logPaths.length === 0;

function existingLog(path) {
    return path && fs.existsSync(path) ? path : null;
}

const baseCpuPath = existingLog(useDefaultLogPaths ? "logs/base-cpu.log" : logPaths[0]);
const arcCpuPath = existingLog(useDefaultLogPaths ? "logs/arc-cpu.log" : logPaths[1]);
const baseMemoryPath = existingLog(useDefaultLogPaths ? "logs/base-memory.log" : logPaths[2]);
const arcMemoryPath = existingLog(useDefaultLogPaths ? "logs/arc-memory.log" : logPaths[3]);

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
        "| payload | users | baseline vec | arc + rx | gain |",
        "|---|---:|---:|---:|---:|",
    ];

    for (const base of orderedRows(baseRows)) {
        const arc = arcRows.get(base.key);
        if (!arc) {
            throw new Error(`missing arc CPU row for ${base.key}`);
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
        "| payload | users | baseline vec | arc + rx | saved |",
        "|---|---:|---:|---:|---:|",
    ];

    for (const base of orderedRows(baseRows)) {
        const arc = arcRows.get(base.key);
        if (!arc) {
            throw new Error(`missing arc allocation row for ${base.key}`);
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
    if (functionName === "rtp_event_fanout") {
        return {
            key: "rtp_event_fanout",
            label: "RTP event fanout",
            order: 0,
        };
    }
    if (functionName === "full_relay") {
        return {
            key: "full_relay",
            label: "full relay",
            order: 1,
        };
    }
    return null;
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

function bothMetricValues(metricDiff) {
    const both = metricDiff?.metrics?.Both;
    if (!Array.isArray(both) || both.length !== 2) {
        return null;
    }
    return {
        arc: metricValue(both[0]),
        base: metricValue(both[1]),
    };
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

        const instructions = bothMetricValues(callgrindSummary(summary)?.Ir);
        if (!instructions) {
            continue;
        }

        const scenario = parseCallgrindId(summary.id);
        rows.push({
            ...scenario,
            group: group.label,
            groupKey: group.key,
            groupOrder: group.order,
            base: instructions.base,
            arc: instructions.arc,
        });
    }

    return rows.sort(
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
        "| path | payload | users | baseline vec | arc + rx | gain |",
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
const hasCpu = Boolean(baseCpuPath && arcCpuPath);
const hasMemory = Boolean(baseMemoryPath && arcMemoryPath);
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
          }, measurement time ${process.env.MEASUREMENT_TIME ?? "criterion default"}s`;
const sections = [
    `settings: allocator ${process.env.ALLOCATOR ?? "system"}, fanout users ${configuredFanout()}, ${criterionSettings}, ${
        process.env.FULL_RELAY_CALLGRIND_ROUNDS ?? "default"
    } inbound packets per Callgrind run`,
];

if (hasCpu) {
    const baseCpu = parseCpu(baseCpuPath, "copied_vec");
    const arcCpu = parseCpu(arcCpuPath, "shared_arc");
    sections.push("", cpuTable(baseCpu, arcCpu));
}

if (hasMemory) {
    const baseMemory = parseMemory(baseMemoryPath, "copied_vec");
    const arcMemory = parseMemory(arcMemoryPath, "shared_arc");
    sections.push("", memoryTable(baseMemory, arcMemory));
}

if (callgrindRows.length > 0) {
    sections.push("", callgrindTable(callgrindRows));
}

console.log(sections.join("\n"));
