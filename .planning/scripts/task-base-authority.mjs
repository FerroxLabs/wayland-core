#!/usr/bin/env node

import fs from "node:fs";

const [operation, file, ...values] = process.argv.slice(2);
const oid = /^[0-9a-f]{40}(?:[0-9a-f]{24})?$/;
const noFollow = fs.constants.O_NOFOLLOW;

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

function parse(bytes) {
  const text = bytes.toString("utf8");
  const match = text.match(/^([0-9a-f]+)\n([0-9a-f]+)\n$/);
  if (!match || !oid.test(match[1]) || !oid.test(match[2])) {
    throw new Error("authority object must contain exactly two exact lowercase object IDs");
  }
  return `${match[1]}\n${match[2]}\n`;
}

if (!file || (operation !== "capture" && operation !== "read")) {
  throw new Error("usage: task-base-authority.mjs capture <file> <commit> <tree> | read <file> [octal-mode]");
}

if (operation === "capture") {
  if (values.length !== 2 || !oid.test(values[0]) || !oid.test(values[1])) {
    throw new Error("capture requires exact commit and tree object IDs");
  }
  const expected = `${values[0]}\n${values[1]}\n`;
  let fd;
  try {
    fd = fs.openSync(
      file,
      fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_EXCL | noFollow,
      0o400,
    );
  } catch (error) {
    if (error?.code !== "EEXIST") throw error;
    const existingFd = fs.openSync(
      file,
      fs.constants.O_RDONLY | fs.constants.O_NONBLOCK | noFollow,
    );
    try {
      requireRegularMode(existingFd, 0o400);
      if (parse(fs.readFileSync(existingFd)) !== expected) {
        throw new Error("existing authority object does not match the requested tuple");
      }
      process.stdout.write(expected);
    } finally {
      fs.closeSync(existingFd);
    }
    process.exit(0);
  }
  try {
    fs.fchmodSync(fd, 0o400);
    requireRegularMode(fd, 0o400);
    fs.writeFileSync(fd, expected, { encoding: "utf8" });
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  process.exit(0);
}

const expectedMode = values.length === 0 ? 0o400 : Number.parseInt(values[0], 8);
if (values.length > 1 || !Number.isInteger(expectedMode)) {
  throw new Error("read accepts at most one octal mode");
}
const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK | noFollow);
try {
  requireRegularMode(fd, expectedMode);
  process.stdout.write(parse(fs.readFileSync(fd)));
} finally {
  fs.closeSync(fd);
}
