#!/usr/bin/env node

import fs from "node:fs";

const [beforeDir, afterDir] = process.argv.slice(2);

if (!beforeDir || !afterDir) {
    throw new Error("usage: summarize_idle_drain_callgrind.mjs <before-gungraun-dir> <after-gungraun-dir>");
}

function read(path) {
    return fs.readFileSync(path, "utf8");
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
    return null;
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

        const values = Object.values(metrics).filter((value) => metricValue(value) !== null);
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

function scenarioLabel(id) {
    if (id === "one_session") {
        return "1 session";
    }
    if (id === "configured_sessions") {
        return `${configuredSessions()} sessions`;
    }
    throw new Error(`unexpected idle drain scenario: ${id}`);
}

function scenarioOrder(id) {
    if (id === "one_session") {
        return 0;
    }
    if (id === "configured_sessions") {
        return 1;
    }
    return 2;
}

function configuredSessions() {
    const value = Number(process.env.IDLE_DRAIN_SESSIONS ?? "128");
    if (Number.isInteger(value) && value > 0) {
        return value;
    }
    return 128;
}

function configuredRounds() {
    const value = Number(process.env.IDLE_DRAIN_ROUNDS ?? "1");
    if (Number.isInteger(value) && value > 0) {
        return value;
    }
    return 1;
}

function parseRows(dir) {
    if (!fs.existsSync(dir)) {
        throw new Error(`missing Callgrind output directory: ${dir}`);
    }

    const rows = new Map();
    for (const path of walkFiles(dir)) {
        if (!path.endsWith("/summary.json")) {
            continue;
        }

        const summary = JSON.parse(read(path));
        if (summary.function_name !== "idle_poll_sessions" || !summary.id) {
            continue;
        }

        const callgrind = callgrindSummary(summary);
        if (!callgrind) {
            continue;
        }

        const instructions = singleMetricValue(callgrind.Ir);
        const dataReads = singleMetricValue(callgrind.Dr);
        const dataWrites = singleMetricValue(callgrind.Dw);
        const totalReadWrite = singleMetricValue(callgrind.TotalRW);
        const estimatedCycles = singleMetricValue(callgrind.EstimatedCycles);
        if (instructions === null) {
            continue;
        }

        rows.set(summary.id, {
            id: summary.id,
            instructions,
            estimatedCycles,
            dataReadWrite:
                totalReadWrite !== null
                    ? totalReadWrite
                    : dataReads !== null && dataWrites !== null
                      ? dataReads + dataWrites
                      : null,
        });
    }

    if (rows.size === 0) {
        throw new Error(`no idle drain Callgrind summaries found in ${dir}`);
    }
    return rows;
}

function formatCount(value) {
    if (value === null) {
        return "n/a";
    }
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

function formatChange(before, after) {
    if (before === null || after === null || before === 0) {
        return "n/a";
    }
    const change = (after / before - 1) * 100;
    if (Math.abs(change) < 0.005) {
        return "~0.00%";
    }
    return `${change > 0 ? "+" : ""}${change.toFixed(2)}%`;
}

function compareRows(beforeRows, afterRows) {
    const ids = [...beforeRows.keys()].sort((left, right) => scenarioOrder(left) - scenarioOrder(right));
    return ids.map((id) => {
        const before = beforeRows.get(id);
        const after = afterRows.get(id);
        if (!after) {
            throw new Error(`missing after row for ${id}`);
        }
        return { id, before, after };
    });
}

const beforeRows = parseRows(beforeDir);
const afterRows = parseRows(afterDir);
const rows = compareRows(beforeRows, afterRows);
const beforeLabel = process.env.BEFORE_LABEL ?? "before";
const afterLabel = process.env.AFTER_LABEL ?? "after";
const sections = [
    `settings: ${configuredSessions()} idle RTP-mode sessions, ${configuredRounds()} drain rounds per run`,
    `comparison: ${beforeLabel} -> ${afterLabel}`,
    "",
    "| scenario | before instructions | after instructions | instruction change | before cycles | after cycles | cycle change | before read+write | after read+write | read+write change |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
];

for (const row of rows) {
    sections.push(
        `| ${scenarioLabel(row.id)} | ${formatCount(row.before.instructions)} | ${formatCount(
            row.after.instructions,
        )} | ${formatChange(row.before.instructions, row.after.instructions)} | ${formatCount(
            row.before.estimatedCycles,
        )} | ${formatCount(row.after.estimatedCycles)} | ${formatChange(
            row.before.estimatedCycles,
            row.after.estimatedCycles,
        )} | ${formatCount(row.before.dataReadWrite)} | ${formatCount(
            row.after.dataReadWrite,
        )} | ${formatChange(row.before.dataReadWrite, row.after.dataReadWrite)} |`,
    );
}

console.log(sections.join("\n"));
