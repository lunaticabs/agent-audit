#!/usr/bin/env node

import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

const IMAGE_PROJECT_ROOT = "/opt/agent-audit";
const DEFAULT_CODEX_HOME = "/root/.codex";
const DEFAULT_CODEX_RUNNER_DIR = "/opt/agent-audit/codex-runner";
const CODEX_RUNNER_DIR =
  process.env.CODEX_RUNNER_DIR?.trim() || DEFAULT_CODEX_RUNNER_DIR;
const CODEX_BIN =
  process.env.CODEX_BIN?.trim() ||
  path.join(CODEX_RUNNER_DIR, "node_modules", ".bin", "codex");
const ENV_FILE = path.join(IMAGE_PROJECT_ROOT, ".env");
const MAX_STRING_LENGTH = 2_000;
const PRODUCTION_DATA_ENV_KEYS = [
  "AGENT_AUDIT_RPC_URL",
  "AGENT_AUDIT_SOURCE_API_BASE",
  "AGENT_AUDIT_SOURCE_API_KEY",
  "AGENT_AUDIT_MONGODB_URI",
  "MONGODB_URI",
  "ETH_RPC_URL",
  "FOUNDRY_ETH_RPC_URL",
];

function usage() {
  return [
    "usage: docker run ... [--prompt <text>]",
    "",
    "prompt sources (highest priority first):",
    "  1. FULL_PROMPT environment variable",
    "  2. --prompt <text> CLI argument",
    "",
    "optional:",
    "  --prompt <text>                Full prompt text for local/manual runs",
    "  CODEX_WORKDIR=<path>           Run Codex in this directory instead of the bundled project root",
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
    prompt: null,
    help: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "-h" || arg === "--help") {
      args.help = true;
      continue;
    }

    if (arg.startsWith("--prompt=")) {
      args.prompt = arg.slice("--prompt=".length);
      continue;
    }

    if (arg === "--prompt") {
      const value = argv[i + 1];
      if (!value || value.startsWith("--")) {
        return { error: "--prompt requires a value" };
      }
      args.prompt = value;
      i += 1;
      continue;
    }

    return { error: `unknown argument: ${arg}` };
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

function scrubProductionDataEnv() {
  const removed = [];
  for (const key of PRODUCTION_DATA_ENV_KEYS) {
    if (Object.prototype.hasOwnProperty.call(process.env, key)) {
      delete process.env[key];
      removed.push(key);
    }
  }
  return removed;
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

function resolvePathSetting(value, fallback) {
  const trimmed = typeof value === "string" ? value.trim() : "";
  const resolved = trimmed === "" ? fallback : trimmed;
  return path.isAbsolute(resolved) ? resolved : path.resolve(resolved);
}

function resolveRuntimePaths(env = process.env) {
  const projectRoot = resolvePathSetting(env.AGENT_AUDIT_PROJECT_ROOT, IMAGE_PROJECT_ROOT);
  const codexHome = resolvePathSetting(env.CODEX_HOME, DEFAULT_CODEX_HOME);
  const workingDirectory = resolvePathSetting(env.CODEX_WORKDIR, projectRoot);
  const bundledConfig = path.join(projectRoot, ".codex", "config.toml");
  const bundledSkills = path.join(projectRoot, ".codex", "skills");
  const codexConfig = path.join(codexHome, "config.toml");
  const codexSkills = path.join(codexHome, "skills");

  return {
    projectRoot,
    workingDirectory,
    codexHome,
    bundledConfig,
    bundledSkills,
    codexConfig,
    codexSkills,
  };
}

function ensureRuntime() {
  const runtime = resolveRuntimePaths();
  process.env.AGENT_AUDIT_PROJECT_ROOT = runtime.projectRoot;
  process.env.CODEX_HOME = runtime.codexHome;

  const {
    projectRoot,
    workingDirectory,
    codexHome,
    bundledConfig,
    bundledSkills,
    codexConfig,
    codexSkills,
  } = runtime;

  if (!fs.existsSync(projectRoot)) {
    throw new Error(`agent-audit project root not found at ${projectRoot}`);
  }
  if (!fs.existsSync(workingDirectory)) {
    throw new Error(`Codex working directory not found at ${workingDirectory}`);
  }

  if (workingDirectory === projectRoot || process.env.AGENT_AUDIT_CREATE_RUNS_DIR === "1") {
    fs.mkdirSync(path.join(projectRoot, "runs"), { recursive: true });
  }
  fs.mkdirSync(codexHome, { recursive: true });

  if (!fs.existsSync(codexConfig) && fs.existsSync(bundledConfig)) {
    fs.copyFileSync(bundledConfig, codexConfig);
  }

  if (!fs.existsSync(codexSkills) && fs.existsSync(bundledSkills)) {
    fs.cpSync(bundledSkills, codexSkills, {
      recursive: true,
      force: false,
      errorOnExist: false,
    });
  }

  if (!fs.existsSync(CODEX_BIN)) {
    throw new Error(`codex binary not found at ${CODEX_BIN}`);
  }

  process.chdir(workingDirectory);

  return runtime;
}

function resolvePrompt(args, env = process.env) {
  const envPrompt = typeof env.FULL_PROMPT === "string" ? env.FULL_PROMPT.trim() : "";
  if (envPrompt !== "") {
    return {
      prompt: env.FULL_PROMPT,
      source: "FULL_PROMPT",
    };
  }

  const cliPrompt = typeof args.prompt === "string" ? args.prompt.trim() : "";
  if (cliPrompt !== "") {
    return {
      prompt: args.prompt,
      source: "--prompt",
    };
  }

  return {
    error: "FULL_PROMPT or --prompt is required",
  };
}

async function loadCodexSdk() {
  const requireFromRunner = createRequire(path.join(CODEX_RUNNER_DIR, "package.json"));
  const sdkPath = requireFromRunner.resolve("@openai/codex-sdk");
  const { Codex } = await import(pathToFileURL(sdkPath).href);
  return Codex;
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
  const scrubbedEnvKeys = scrubProductionDataEnv();
  const promptInfo = resolvePrompt(args);
  if (promptInfo.error) {
    const error = new Error(promptInfo.error);
    error.name = "UsageError";
    throw error;
  }

  const runtime = ensureRuntime();
  const { prompt, source: promptSource } = promptInfo;

  infoLog("starting audit", {
    task_id: process.env.TASK_ID,
    prompt_source: promptSource,
    cwd: runtime.workingDirectory,
    project_root: runtime.projectRoot,
    codex_home: runtime.codexHome,
    codex_config: runtime.codexConfig,
  });
  infoLog("loaded environment", {
    dotenv_path: ENV_FILE,
    dotenv_loaded: envInfo.loaded,
    dotenv_keys: envInfo.keys,
    scrubbed_keys: scrubbedEnvKeys,
  });
  infoLog("prepared prompt", {
    prompt,
  });

  const Codex = await loadCodexSdk();
  const codex = new Codex({
    codexPathOverride: CODEX_BIN,
  });

  const thread = await codex.startThread({
    approvalPolicy: "never",
    sandboxMode: "danger-full-access",
    workingDirectory: runtime.workingDirectory,
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
    if (error?.name === "UsageError") {
      errorLog("invalid arguments", {
        error_type: "usage_error",
        message: error?.message || String(error),
      });
      process.stderr.write(`${usage()}\n`);
      return 2;
    }

    errorLog("audit failed", {
      error_type: error?.name || "Error",
      message: error?.message || String(error),
    });
    return 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const exitCode = await main();
  process.exitCode = exitCode;
}

export { parseArgs, resolvePrompt, resolveRuntimePaths, usage };
