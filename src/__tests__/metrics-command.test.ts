import { Command } from "commander";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  readMetricDefinition,
  registerMetricsCommand,
} from "../commands/metrics.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { force: true, recursive: true });
  }
});

describe("metrics command", () => {
  it("requires native QuerySpec files and exposes no legacy authoring flags", () => {
    const program = new Command();
    registerMetricsCommand(program);

    const metrics = program.commands.find(
      (command) => command.name() === "metrics",
    );
    const create = metrics?.commands.find(
      (command) => command.name() === "create",
    );
    const update = metrics?.commands.find(
      (command) => command.name() === "update",
    );

    expect(
      create?.options.find((option) => option.long === "--definition")
        ?.mandatory,
    ).toBe(true);
    expect(create?.options.map((option) => option.long)).not.toContain(
      "--type",
    );
    expect(create?.options.map((option) => option.long)).not.toContain(
      "--formula",
    );
    expect(update?.options.map((option) => option.long)).not.toContain(
      "--formula",
    );
  });

  it("reads a QuerySpec definition without changing its payload", () => {
    const directory = mkdtempSync(join(tmpdir(), "vendo-metric-definition-"));
    temporaryDirectories.push(directory);
    const path = join(directory, "metric.query.json");
    const definition = {
      version: 2,
      reportType: "segmentation",
      metricOutput: { kind: "measure", measureId: "m_a" },
    };
    writeFileSync(path, JSON.stringify(definition));

    expect(readMetricDefinition(path)).toEqual(definition);
  });

  it("reports the file path when a definition cannot be read", () => {
    expect(() => readMetricDefinition("/missing/metric.query.json")).toThrow(
      "Failed to read Metric definition /missing/metric.query.json",
    );
  });
});
