#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [operation, objectPath, ...values] = process.argv.slice(2);
const oid = /^[0-9a-f]{40}(?:[0-9a-f]{24})?$/;
const planId = /^[0-9]+(?:\.[0-9]+)?-[0-9]+$/;
const generationId = /^g-[0-9a-f]{64}$/;
const noFollow = fs.constants.O_NOFOLLOW;
const taskSchema = "wayland-core.task-base.v1";
const dispositionSchema = ["wayland-core.ta", "s", "k", "-base-disposition.v1"].join("");

if (typeof noFollow !== "number") {
  throw new Error("O_NOFOLLOW is unavailable on this platform");
}

function requireRegularMode(fd, mode) {
  const stat = fs.fstatSync(fd);
  if (!stat.isFile()) throw new Error("authority object is not a regular file");
  if ((stat.mode & 0o777) !== mode) {
    throw new Error(`authority object mode must be ${mode.toString(8)}`);
  }
}

function requireDirectory(directory, mode) {
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) {
    throw new Error("task authority root is not a real directory");
  }
  if (mode !== undefined && (stat.mode & 0o777) !== mode) {
    throw new Error(`task authority directory mode must be ${mode.toString(8)}`);
  }
}

function readRegular(file, mode) {
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK | noFollow);
  try {
    requireRegularMode(fd, mode);
    return fs.readFileSync(fd);
  } finally {
    fs.closeSync(fd);
  }
}

function parseTuple(bytes) {
  const text = bytes.toString("utf8");
  const match = text.match(/^([0-9a-f]+)\n([0-9a-f]+)\n$/);
  if (!match || !oid.test(match[1]) || !oid.test(match[2])) {
    throw new Error("authority object must contain exactly two exact lowercase object IDs");
  }
  return { commit: match[1], tree: match[2], text: `${match[1]}\n${match[2]}\n` };
}

function syncDirectory(directory) {
  let fd;
  try {
    fd = fs.openSync(directory, fs.constants.O_RDONLY | noFollow);
    fs.fsyncSync(fd);
  } catch (error) {
    if (!["EINVAL", "ENOTSUP", "EISDIR", "EPERM"].includes(error?.code)) throw error;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

function atomicNoClobber(file, bytes, mode) {
  const directory = path.dirname(file);
  requireDirectory(directory);
  const temporary = path.join(
    directory,
    `.${path.basename(file)}.tmp-${process.pid}-${crypto.randomBytes(16).toString("hex")}`,
  );
  let fd;
  try {
    fd = fs.openSync(
      temporary,
      fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_EXCL | noFollow,
      mode,
    );
    fs.fchmodSync(fd, mode);
    requireRegularMode(fd, mode);
    fs.writeFileSync(fd, bytes);
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    fd = undefined;

    const targetSuffix = process.env.WAYLAND_TEST_TASK_AUTHORITY_TARGET_SUFFIX;
    const targetMatches = !targetSuffix || file.endsWith(targetSuffix);
    if (
      targetMatches &&
      process.env.WAYLAND_TEST_TASK_AUTHORITY_KILL_BEFORE_PUBLISH === "1"
    ) {
      process.kill(process.pid, "SIGKILL");
    }
    if (
      targetMatches &&
      process.env.WAYLAND_TEST_TASK_AUTHORITY_FAIL_BEFORE_PUBLISH === "1"
    ) {
      throw new Error("injected interruption before authority publication");
    }

    try {
      fs.linkSync(temporary, file);
      syncDirectory(directory);
      if (
        targetMatches &&
        process.env.WAYLAND_TEST_TASK_AUTHORITY_FAIL_AFTER_PUBLISH === "1"
      ) {
        throw new Error("injected interruption after authority publication");
      }
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      const existing = readRegular(file, mode);
      if (!existing.equals(Buffer.from(bytes))) {
        throw new Error("existing authority object does not match the requested bytes");
      }
    }
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
    try {
      fs.unlinkSync(temporary);
      syncDirectory(directory);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
}

function canonicalJson(value) {
  return `${JSON.stringify(value)}\n`;
}

function exactKeys(value, expected, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const observed = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(observed) !== JSON.stringify(wanted)) {
    throw new Error(`${label} has unknown or missing fields`);
  }
}

function parseJsonFile(file, mode, label) {
  const bytes = readRegular(file, mode);
  let value;
  try {
    value = JSON.parse(bytes.toString("utf8"));
  } catch {
    throw new Error(`${label} is malformed JSON`);
  }
  if (!bytes.equals(Buffer.from(canonicalJson(value)))) {
    throw new Error(`${label} is not canonical JSON`);
  }
  return value;
}

function deriveGeneration(plan, parent, commit, tree) {
  return `g-${crypto
    .createHash("sha256")
    .update(`${taskSchema}\0${plan}\0${parent}\0${commit}\0${tree}`)
    .digest("hex")}`;
}

function taskPaths(root, generation) {
  const generationRoot = path.join(root, "generations", generation);
  return {
    generationRoot,
    base: path.join(generationRoot, "base"),
    disposition: path.join(generationRoot, "disposition.json"),
  };
}

function ensureTaskRoot(root) {
  requireDirectory(path.dirname(root));
  if (!fs.existsSync(root)) {
    try {
      fs.mkdirSync(root, { mode: 0o700 });
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
    }
  }
  requireDirectory(root, 0o700);
  const generations = path.join(root, "generations");
  if (!fs.existsSync(generations)) {
    try {
      fs.mkdirSync(generations, { mode: 0o700 });
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
    }
  }
  requireDirectory(generations, 0o700);
  syncDirectory(path.dirname(root));
}

function createGeneration(root, generation, commit, tree) {
  const paths = taskPaths(root, generation);
  if (!fs.existsSync(paths.generationRoot)) {
    try {
      fs.mkdirSync(paths.generationRoot, { mode: 0o700 });
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
    }
  }
  requireDirectory(paths.generationRoot, 0o700);
  syncDirectory(path.dirname(paths.generationRoot));
  atomicNoClobber(paths.base, `${commit}\n${tree}\n`, 0o400);
  return paths;
}

function readState(root, expectedPlan) {
  requireDirectory(root, 0o700);
  requireDirectory(path.join(root, "generations"), 0o700);
  const state = parseJsonFile(path.join(root, "state.json"), 0o400, "task authority state");
  exactKeys(state, ["schema", "plan", "root_generation"], "task authority state");
  if (state.schema !== taskSchema || !planId.test(state.plan) || !generationId.test(state.root_generation)) {
    throw new Error("task authority state has invalid values");
  }
  if (expectedPlan !== undefined && state.plan !== expectedPlan) {
    throw new Error("task authority plan does not match the requested plan");
  }
  return state;
}

function readGeneration(root, plan, generation) {
  if (!generationId.test(generation)) throw new Error("invalid task generation ID");
  const paths = taskPaths(root, generation);
  requireDirectory(paths.generationRoot, 0o700);
  const tuple = parseTuple(readRegular(paths.base, 0o400));
  let disposition = null;
  const dispositionStat = fs.lstatSync(paths.disposition, { throwIfNoEntry: false });
  if (dispositionStat !== undefined) {
    if (!dispositionStat.isFile() || dispositionStat.isSymbolicLink()) {
      throw new Error("task generation disposition is not a regular file");
    }
    {
      disposition = parseJsonFile(paths.disposition, 0o400, "task generation disposition");
      const common = ["schema", "plan", "generation", "status", "head"];
      const keys = disposition.status === "abandoned" ? [...common, "next_generation"] : common;
      exactKeys(disposition, keys, "task generation disposition");
      if (
        disposition.schema !== dispositionSchema ||
        disposition.plan !== plan ||
        disposition.generation !== generation ||
        !oid.test(disposition.head) ||
        !["abandoned", "completed"].includes(disposition.status) ||
        (disposition.status === "abandoned" && !generationId.test(disposition.next_generation))
      ) {
        throw new Error("task generation disposition has invalid values");
      }
    }
  }
  return { ...tuple, disposition };
}

function resolveTaskTip(root, expectedPlan) {
  const state = readState(root, expectedPlan);
  let generation = state.root_generation;
  let parent = "root";
  let expectedCommit = null;
  const visited = new Set();
  for (let depth = 0; depth < 1000; depth += 1) {
    if (visited.has(generation)) throw new Error("task generation chain contains a cycle");
    visited.add(generation);
    const current = readGeneration(root, state.plan, generation);
    const derived = deriveGeneration(state.plan, parent, current.commit, current.tree);
    if (generation !== derived) {
      throw new Error("task generation ID does not bind its plan, parent, commit, and tree");
    }
    if (expectedCommit !== null && current.commit !== expectedCommit) {
      throw new Error("task generation successor does not match the abandoned disposition head");
    }
    if (!current.disposition || current.disposition.status === "completed") {
      return { plan: state.plan, generation, ...current };
    }
    parent = generation;
    expectedCommit = current.disposition.head;
    generation = current.disposition.next_generation;
  }
  throw new Error("task generation chain exceeds the safety bound");
}

function resolveTask(root, expectedPlan) {
  const current = resolveTaskTip(root, expectedPlan);
  if (current.disposition?.status === "completed") {
    throw new Error("task generation is complete; no active generation remains");
  }
  return current;
}

function requireTuple(valuesToCheck, label) {
  if (valuesToCheck.length !== 2 || !oid.test(valuesToCheck[0]) || !oid.test(valuesToCheck[1])) {
    throw new Error(`${label} requires exact commit and tree object IDs`);
  }
}

function outputTask(current) {
  process.stdout.write(`${current.commit}\n${current.tree}\n${current.generation}\n`);
}

if (!objectPath) {
  throw new Error("missing authority object path");
}

if (operation === "capture") {
  requireTuple(values, "capture");
  atomicNoClobber(objectPath, `${values[0]}\n${values[1]}\n`, 0o400);
  process.exit(0);
}

if (operation === "read") {
  if (fs.existsSync(objectPath) && fs.lstatSync(objectPath).isDirectory()) {
    const current = resolveTask(objectPath);
    process.stdout.write(`${current.commit}\n${current.tree}\n`);
    process.exit(0);
  }
  const expectedMode = values.length === 0 ? 0o400 : Number.parseInt(values[0], 8);
  if (values.length > 1 || !Number.isInteger(expectedMode)) {
    throw new Error("read accepts at most one octal mode");
  }
  process.stdout.write(parseTuple(readRegular(objectPath, expectedMode)).text);
  process.exit(0);
}

if (operation === "task-begin") {
  if (values.length !== 3 || !planId.test(values[0])) {
    throw new Error("task-begin requires plan ID, commit, and tree");
  }
  requireTuple(values.slice(1), "task-begin");
  const [plan, commit, tree] = values;
  ensureTaskRoot(objectPath);
  const statePath = path.join(objectPath, "state.json");
  if (fs.existsSync(statePath)) {
    const state = readState(objectPath, plan);
    const root = readGeneration(objectPath, plan, state.root_generation);
    if (root.commit !== commit || root.tree !== tree) {
      throw new Error("existing task authority root does not match the requested tuple");
    }
    outputTask(resolveTask(objectPath, plan));
    process.exit(0);
  }
  const generation = deriveGeneration(plan, "root", commit, tree);
  createGeneration(objectPath, generation, commit, tree);
  atomicNoClobber(
    statePath,
    canonicalJson({ schema: taskSchema, plan, root_generation: generation }),
    0o400,
  );
  outputTask(resolveTask(objectPath, plan));
  process.exit(0);
}

if (operation === "task-current") {
  if (values.length !== 1 || !planId.test(values[0])) {
    throw new Error("task-current requires a plan ID");
  }
  outputTask(resolveTask(objectPath, values[0]));
  process.exit(0);
}

if (operation === "task-tip") {
  if (values.length !== 1 || !planId.test(values[0])) {
    throw new Error("task-tip requires a plan ID");
  }
  outputTask(resolveTaskTip(objectPath, values[0]));
  process.exit(0);
}

if (operation === "task-start-fresh") {
  if (
    values.length !== 4 ||
    !planId.test(values[0]) ||
    !generationId.test(values[1]) ||
    !oid.test(values[2]) ||
    !oid.test(values[3])
  ) {
    throw new Error("task-start-fresh requires plan ID, old generation, reconciled commit, and tree");
  }
  const [plan, oldGeneration, commit, tree] = values;
  const nextGeneration = deriveGeneration(plan, oldGeneration, commit, tree);
  const current = resolveTask(objectPath, plan);
  if (current.generation !== oldGeneration) {
    const old = readGeneration(objectPath, plan, oldGeneration);
    if (
      current.generation === nextGeneration &&
      current.commit === commit &&
      current.tree === tree &&
      old.disposition?.status === "abandoned" &&
      old.disposition.next_generation === nextGeneration &&
      old.disposition.head === commit
    ) {
      outputTask(current);
      process.exit(0);
    }
    throw new Error("named old generation is not the active task generation");
  }
  createGeneration(objectPath, nextGeneration, commit, tree);
  atomicNoClobber(
    taskPaths(objectPath, oldGeneration).disposition,
    canonicalJson({
      schema: dispositionSchema,
      plan,
      generation: oldGeneration,
      status: "abandoned",
      head: commit,
      next_generation: nextGeneration,
    }),
    0o400,
  );
  outputTask(resolveTask(objectPath, plan));
  process.exit(0);
}

if (operation === "task-complete") {
  if (
    values.length !== 3 ||
    !planId.test(values[0]) ||
    !generationId.test(values[1]) ||
    !oid.test(values[2])
  ) {
    throw new Error("task-complete requires plan ID, active generation, and reconciled commit");
  }
  const [plan, generation, commit] = values;
  const current = resolveTaskTip(objectPath, plan);
  if (current.generation !== generation) {
    throw new Error("named generation is not the active task generation");
  }
  if (current.disposition?.status === "completed") {
    if (current.disposition.head !== commit) {
      throw new Error("completed task generation does not match the requested commit");
    }
    process.stdout.write(`completed\n${generation}\n${commit}\n`);
    process.exit(0);
  }
  atomicNoClobber(
    taskPaths(objectPath, generation).disposition,
    canonicalJson({
      schema: dispositionSchema,
      plan,
      generation,
      status: "completed",
      head: commit,
    }),
    0o400,
  );
  process.stdout.write(`completed\n${generation}\n${commit}\n`);
  process.exit(0);
}

throw new Error(
  "usage: task-base-authority.mjs capture <file> <commit> <tree> | read <file> [octal-mode] | task-begin <dir> <plan> <commit> <tree> | task-current <dir> <plan> | task-tip <dir> <plan> | task-start-fresh <dir> <plan> <old-generation> <commit> <tree> | task-complete <dir> <plan> <generation> <commit>",
);
