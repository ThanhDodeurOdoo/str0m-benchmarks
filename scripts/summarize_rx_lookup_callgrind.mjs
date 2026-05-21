#!/usr/bin/env node

import fs from "node:fs";

const [beforeDir, afterDir] = process.argv.slice(2);

if (!beforeDir || !afterDir) {
    throw new Error("usage: summarize_rx_lookup_callgrind.mjs <before-iai-dir> <after-iai-dir>");
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
    if (id === "one_stream") {
        return "1 stream";
    }
    if (id === "configured_streams") {
        return `${configuredStreams()} streams`;
    }
    throw new Error(`unexpected rx lookup scenario: ${id}`);
}

function scenarioOrder(id) {
    if (id === "one_stream") {
        return 0;
    }
    if (id === "configured_streams") {
        return 1;
    }
    return 2;
}

function configuredStreams() {
    const value = Number(process.env.RX_LOOKUP_STREAMS ?? "512");
    if (Number.isInteger(value) && value > 0) {
        return value;
    }
    return 512;
}

function configuredPackets() {
    const value = Number(process.env.RX_LOOKUP_PACKETS ?? "256");
    if (Number.isInteger(value) && value > 0) {
        return value;
    }
    return 256;
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
        if (summary.function_name !== "receive_packets" || !summary.id) {
            continue;
        }

        const callgrind = callgrindSummary(summary);
        if (!callgrind) {
            continue;
        }

        const instructions = singleMetricValue(callgrind.Ir);
        const dataReads = singleMetricValue(callgrind.Dr);
        const dataWrites = singleMetricValue(callgrind.Dw);
        if (instructions === null) {
            continue;
        }

        rows.set(summary.id, {
            id: summary.id,
            instructions,
            dataReadWrite: dataReads !== null && dataWrites !== null ? dataReads + dataWrites : null,
        });
    }

    if (rows.size === 0) {
        throw new Error(`no rx lookup Callgrind summaries found in ${dir}`);
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

function formatPercent(before, after) {
    if (before === null || after === null || before === 0) {
        return "n/a";
    }
    const rounded = Math.round((1 - after / before) * 100);
    if (rounded === 0) {
        return "~0%";
    }
    return `${rounded < 0 ? "~-" : "~"}${Math.abs(rounded)}%`;
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
const sections = [
    `settings: ${configuredStreams()} seeded RTP-mode streams, ${configuredPackets()} received packets per run`,
    "",
    "| scenario | baseline instructions | fixed instructions | instruction gain | baseline data read+write | fixed data read+write | data gain |",
    "|---|---:|---:|---:|---:|---:|---:|",
];

for (const row of rows) {
    sections.push(
        `| ${scenarioLabel(row.id)} | ${formatCount(row.before.instructions)} | ${formatCount(
            row.after.instructions,
        )} | ${formatPercent(row.before.instructions, row.after.instructions)} | ${formatCount(
            row.before.dataReadWrite,
        )} | ${formatCount(row.after.dataReadWrite)} | ${formatPercent(
            row.before.dataReadWrite,
            row.after.dataReadWrite,
        )} |`,
    );
}

console.log(sections.join("\n"));
