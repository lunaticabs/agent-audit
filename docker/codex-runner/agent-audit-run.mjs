#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import { Codex } from "@openai/codex-sdk";

const IMAGE_PROJECT_ROOT = "/opt/agent-audit";
const DEFAULT_CODEX_HOME = "/root/.codex";
const CODEX_RUNNER_DIR = "/opt/agent-audit/codex-runner";
const CODEX_BIN = path.join(CODEX_RUNNER_DIR, "node_modules", ".bin", "codex");
const ENV_FILE = path.join(IMAGE_PROJECT_ROOT, ".env");
const MAX_STRING_LENGTH = 2_000;

function usage() {
  return [
    "usage: docker run ... --address <contract_address> [--chain <chain>] [--instructions <text>]",
    "",
    "required:",
    "  --address <contract_address>   Contract address to audit",
    "",
    "optional:",
    "  --chain <chain>                Chain alias (default: eth)",
    "  --instructions <text>          Extra instructions appended to the Codex prompt",
    "  -h, --help                     Show this help message",
  ].join("\n");
}

function writeLogLine(stream, level, message, details = {}) {
  const detailText = formatDetails(details);
  const line = `[${new Date().toISOString()}] ${level.toUpperCase()} ${message}`;
  stream.write(detailText === "" ? `${line}\n` : `${line} ${detailText}\n`);
}

function infoLog(message, details = {}) {
  writeLogLine(process.stdout, "info", message, details);
}

function errorLog(message, details = {}) {
  writeLogLine(process.stderr, "error", message, details);
}

function writeTextBlock(title, text) {
  const trimmed = text.trim();
  if (trimmed === "") {
    return;
  }

  process.stdout.write(`\n=== ${title} ===\n`);
  process.stdout.write(`${trimmed}\n`);
  process.stdout.write(`=== end ${title.toLowerCase()} ===\n\n`);
}

function formatDetails(details) {
  const pairs = [];
  for (const [key, rawValue] of Object.entries(details)) {
    if (rawValue === undefined || rawValue === null || rawValue === "") {
      continue;
    }

    const value = formatDetailValue(rawValue);
    if (value === "") {
      continue;
    }
    pairs.push(`${key}=${value}`);
  }
  return pairs.join(" ");
}

function formatDetailValue(value) {
  if (typeof value === "string") {
    return quoteIfNeeded(truncateString(value));
  }

  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }

  if (typeof value === "bigint") {
    return value.toString();
  }

  if (Array.isArray(value)) {
    if (value.length === 0) {
      return "";
    }
    return quoteIfNeeded(value.map((item) => truncateString(String(item))).join(","));
  }

  if (typeof value === "object") {
    const flattened = flattenObject(value);
    if (flattened.length === 0) {
      return "";
    }
    return quoteIfNeeded(flattened.join(", "));
  }

  return quoteIfNeeded(truncateString(String(value)));
}

function flattenObject(value, prefix = "") {
  const output = [];
  for (const [key, rawItem] of Object.entries(value)) {
    const itemKey = prefix === "" ? key : `${prefix}.${key}`;
    if (rawItem === null || rawItem === undefined || rawItem === "") {
      continue;
    }

    if (
      typeof rawItem === "string" ||
      typeof rawItem === "number" ||
      typeof rawItem === "boolean" ||
      typeof rawItem === "bigint"
    ) {
      output.push(`${itemKey}:${truncateString(String(rawItem))}`);
      continue;
    }

    if (Array.isArray(rawItem)) {
      if (rawItem.length > 0) {
        output.push(`${itemKey}:${truncateString(rawItem.map((item) => String(item)).join("|"))}`);
      }
      continue;
    }

    if (typeof rawItem === "object") {
      output.push(...flattenObject(rawItem, itemKey));
    }
  }
  return output;
}

function quoteIfNeeded(value) {
  if (value === "") {
    return "";
  }
  if (/^[A-Za-z0-9._/:=-]+$/.test(value)) {
    return value;
  }
  return JSON.stringify(value);
}

function truncateString(value) {
  if (value.length <= MAX_STRING_LENGTH) {
    return value;
  }
  return `${value.slice(0, MAX_STRING_LENGTH)}...<truncated ${value.length - MAX_STRING_LENGTH} chars>`;
}

function summarizeEvent(event) {
  const summary = {
    type: event.type,
  };

  if (event.item?.type) {
    summary.item = event.item.type;
  }

  if (event.toolName) {
    summary.tool = event.toolName;
  }

  if (event.callId) {
    summary.call_id = event.callId;
  }

  if (event.command) {
    summary.command = event.command;
  } else if (event.item?.command) {
    summary.command = event.item.command;
  }

  if (event.exitCode !== undefined) {
    summary.exit_code = event.exitCode;
  } else if (event.item?.exitCode !== undefined) {
    summary.exit_code = event.item.exitCode;
  }

  const message = extractFailureMessage(event);
  if (message) {
    summary.message = message;
  }

  return summary;
}

function parseArgs(argv) {
  const args = {
    address: null,
    chain: "eth",
    instructions: "",
    help: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "-h" || arg === "--help") {
      args.help = true;
      continue;
    }

    if (arg.startsWith("--address=")) {
      args.address = arg.slice("--address=".length);
      continue;
    }

    if (arg === "--address") {
      const value = argv[i + 1];
      if (!value || value.startsWith("--")) {
        return { error: "--address requires a value" };
      }
      args.address = value;
      i += 1;
      continue;
    }

    if (arg.startsWith("--chain=")) {
      args.chain = arg.slice("--chain=".length);
      continue;
    }

    if (arg === "--chain") {
      const value = argv[i + 1];
      if (!value || value.startsWith("--")) {
        return { error: "--chain requires a value" };
      }
      args.chain = value;
      i += 1;
      continue;
    }

    if (arg.startsWith("--instructions=")) {
      args.instructions = arg.slice("--instructions=".length);
      continue;
    }

    if (arg === "--instructions") {
      const value = argv[i + 1];
      if (!value || value.startsWith("--")) {
        return { error: "--instructions requires a value" };
      }
      args.instructions = value;
      i += 1;
      continue;
    }

    return { error: `unknown argument: ${arg}` };
  }

  if (!args.help && !args.address) {
    return { error: "--address is required" };
  }

  return { args };
}

function loadDotEnv(filePath) {
  if (!fs.existsSync(filePath)) {
    return { loaded: false, keys: [] };
  }

  const keys = [];
  const content = fs.readFileSync(filePath, "utf8");
  for (const rawLine of content.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (line === "" || line.startsWith("#")) {
      continue;
    }
    if (line.startsWith("export ")) {
      line = line.slice("export ".length).trim();
    }

    const match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/);
    if (!match) {
      continue;
    }

    const [, key, rawValue] = match;
    process.env[key] = parseEnvValue(rawValue);
    keys.push(key);
  }

  return { loaded: true, keys };
}

function parseEnvValue(rawValue) {
  const value = rawValue.trim();
  if (value === "") {
    return "";
  }

  if (value.startsWith('"') && value.endsWith('"')) {
    return value
      .slice(1, -1)
      .replace(/\\n/g, "\n")
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, "\\");
  }

  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1);
  }

  const commentIndex = value.search(/\s#/);
  if (commentIndex === -1) {
    return value;
  }
  return value.slice(0, commentIndex).trimEnd();
}

function ensureRuntime() {
  process.env.CODEX_HOME = process.env.CODEX_HOME?.trim() || DEFAULT_CODEX_HOME;
  process.env.AGENT_AUDIT_PROJECT_ROOT =
    process.env.AGENT_AUDIT_PROJECT_ROOT?.trim() || IMAGE_PROJECT_ROOT;

  const projectRoot = process.env.AGENT_AUDIT_PROJECT_ROOT;
  const codexHome = process.env.CODEX_HOME;
  const bundledConfig = path.join(projectRoot, ".codex", "config.toml");
  const codexConfig = path.join(codexHome, "config.toml");

  fs.mkdirSync(path.join(projectRoot, "runs"), { recursive: true });
  fs.mkdirSync(codexHome, { recursive: true });

  if (!fs.existsSync(codexConfig) && fs.existsSync(bundledConfig)) {
    fs.copyFileSync(bundledConfig, codexConfig);
  }

  if (!fs.existsSync(CODEX_BIN)) {
    throw new Error(`codex binary not found at ${CODEX_BIN}`);
  }

  process.chdir(projectRoot);

  return {
    projectRoot,
    codexHome,
    codexConfig,
  };
}

function buildPrompt(address, chain, instructions) {
  const prompt = `Check AGENTS.md and audit ${address} on ${chain}.`;
  if (!instructions.trim()) {
    return prompt;
  }
  return `${prompt}\n\n${instructions}`;
}

function extractThreadId(event) {
  return event?.threadId ?? event?.thread_id ?? null;
}

function extractFailureMessage(event) {
  if (typeof event?.message === "string" && event.message.trim() !== "") {
    return event.message;
  }
  if (typeof event?.error?.message === "string" && event.error.message.trim() !== "") {
    return event.error.message;
  }
  if (typeof event?.item?.message === "string" && event.item.message.trim() !== "") {
    return event.item.message;
  }
  return null;
}

function extractCompletedResponse(event) {
  if (typeof event?.finalResponse === "string" && event.finalResponse.trim() !== "") {
    return event.finalResponse;
  }
  if (typeof event?.final_response === "string" && event.final_response.trim() !== "") {
    return event.final_response;
  }
  if (
    typeof event?.turn?.finalResponse === "string" &&
    event.turn.finalResponse.trim() !== ""
  ) {
    return event.turn.finalResponse;
  }
  return "";
}

async function runAudit(args) {
  const envInfo = loadDotEnv(ENV_FILE);
  const runtime = ensureRuntime();
  const prompt = buildPrompt(args.address, args.chain, args.instructions);

  infoLog("starting audit", {
    address: args.address,
    chain: args.chain,
    cwd: runtime.projectRoot,
    codex_home: runtime.codexHome,
    codex_config: runtime.codexConfig,
  });
  infoLog("loaded environment", {
    dotenv_path: ENV_FILE,
    dotenv_loaded: envInfo.loaded,
    dotenv_keys: envInfo.keys,
  });
  infoLog("prepared prompt", {
    prompt,
  });

  const codex = new Codex({
    codexPathOverride: CODEX_BIN,
  });

  const thread = await codex.startThread({
    approvalPolicy: "never",
    sandboxMode: "danger-full-access",
    workingDirectory: runtime.projectRoot,
    skipGitRepoCheck: true,
  });

  let threadId = thread.id ?? null;
  if (threadId) {
    infoLog("thread started", {
      thread_id: threadId,
    });
  }

  const { events } = await thread.runStreamed(prompt);
  let finalResponse = "";
  let usage = null;
  let turnCompleted = false;
  let threadStartedLogged = threadId !== null;

  for await (const event of events) {
    infoLog("sdk event", summarizeEvent(event));

    if (!threadStartedLogged) {
      threadId = extractThreadId(event);
      if (threadId) {
        infoLog("thread started", {
          thread_id: threadId,
        });
        threadStartedLogged = true;
      }
    }

    if (event.type === "item.completed") {
      if (event.item?.type === "agent_message") {
        const text = typeof event.item.text === "string" ? event.item.text : "";
        if (text.trim() !== "") {
          finalResponse = text;
          infoLog("assistant message received", {
            thread_id: threadId,
          });
          writeTextBlock("Assistant Output", text);
        }
      } else if (event.item?.type === "error") {
        throw new Error(extractFailureMessage(event) || "Codex reported an item error");
      }
    } else if (event.type === "turn.completed") {
      const completedResponse = extractCompletedResponse(event);
      if (completedResponse !== "") {
        finalResponse = completedResponse;
      }
      usage = event.usage ?? null;
      turnCompleted = true;
    } else if (event.type === "turn.failed" || event.type === "error") {
      throw new Error(extractFailureMessage(event) || "Codex run failed");
    }
  }

  if (!turnCompleted) {
    throw new Error("Codex stream ended before turn.completed");
  }

  if (finalResponse.trim() === "") {
    throw new Error("Codex completed without a final agent message");
  }

  infoLog("audit completed", {
    thread_id: threadId,
    usage,
  });
}

async function main() {
  const parsed = parseArgs(process.argv.slice(2));
  if (parsed.error) {
    errorLog("invalid arguments", {
      error_type: "usage_error",
      message: parsed.error,
    });
    process.stderr.write(`${usage()}\n`);
    return 2;
  }

  if (parsed.args.help) {
    process.stdout.write(`${usage()}\n`);
    return 0;
  }

  try {
    await runAudit(parsed.args);
    return 0;
  } catch (error) {
    errorLog("audit failed", {
      error_type: error?.name || "Error",
      message: error?.message || String(error),
    });
    return 1;
  }
}

const exitCode = await main();
process.exitCode = exitCode;
