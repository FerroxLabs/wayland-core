#!/usr/bin/env node

import { execFileSync } from "node:child_process";

const [reviewCommit, reviewFile, sourceSha, sourceTree, profile] = process.argv.slice(2);
const oid = /^[0-9a-f]{40}(?:[0-9a-f]{24})?$/;
const identity = /^[A-Za-z0-9][A-Za-z0-9._:@/-]{2,127}$/;
const profiles = {
  "f20-09": {
    checks: ["all_severity", "candidate_seal_authority", "interface_sufficiency"],
    deferred: [],
  },
  "f20-11": {
    checks: ["all_severity", "containment_authority", "policy_sufficiency"],
    deferred: [],
  },
  "f20-14": {
    checks: ["all_severity", "evidence_integrity", "integration_authority"],
    deferred: ["native_macos", "native_windows"],
  },
  "f20-15": {
    checks: ["all_severity", "public_lifecycle", "retained_authority"],
    deferred: [],
  },
  "f20-16": {
    checks: ["all_severity", "asvs_level_2", "code_review", "phase_validation"],
    deferred: ["native_macos", "native_windows"],
  },
};

function fail(message) {
  throw new Error(`invalid independent review result: ${message}`);
}

function exactKeys(value, keys, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${label} keys must be exactly: ${expected.join(", ")}`);
  }
}

function exactStrings(value, expected, label) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    fail(`${label} must be a string array`);
  }
  const actual = [...value].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${label} must be exactly: ${expected.join(", ") || "empty"}`);
  }
}

if (!oid.test(reviewCommit ?? "") || !oid.test(sourceSha ?? "") || !oid.test(sourceTree ?? "")) {
  fail("review commit, source commit, and source tree must be exact lowercase object IDs");
}
if (!reviewFile || !profiles[profile]) {
  fail("usage: verify-review-result.mjs <review-commit> <review-file> <source-sha> <source-tree> <f20-09|f20-11|f20-14|f20-15|f20-16>");
}

// The (source_sha, source_tree) pair must be internally consistent: the tree
// argument must be exactly source_sha^{tree}. Otherwise a caller could pass an
// independently-chosen tree that the review JSON then matches, decoupling the
// asserted tree from the asserted commit.
const actualSourceTree = execFileSync("git", ["rev-parse", `${sourceSha}^{tree}`], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
}).trim();
if (actualSourceTree !== sourceTree) {
  fail("source tree argument does not match source commit tree");
}

const bytes = execFileSync("git", ["show", `${reviewCommit}:${reviewFile}`], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});
let result;
try {
  result = JSON.parse(bytes);
} catch {
  fail("review blob must be one schema-validated JSON object");
}

exactKeys(
  result,
  [
    "schema",
    "source_sha",
    "source_tree",
    "source_executor_id",
    "reviewer_id",
    "checks",
    "deferred",
    "findings",
    "evidence",
    "disposition",
  ],
  "result",
);
if (result.schema !== "wayland-core.phase20-independent-review.v1") fail("unknown schema");
if (result.source_sha !== sourceSha || result.source_tree !== sourceTree) {
  fail("source commit/tree do not match the admitted candidate");
}
if (!identity.test(result.source_executor_id ?? "") || !identity.test(result.reviewer_id ?? "")) {
  fail("executor identities must be explicit stable identifiers");
}
if (result.source_executor_id === result.reviewer_id) fail("reviewer must differ from source executor");
if (result.disposition !== "PASS") fail("disposition is not PASS");

const expectedChecks = profiles[profile].checks;
exactKeys(result.checks, expectedChecks, "checks");
for (const check of expectedChecks) {
  if (result.checks[check] !== "PASS") fail(`${check} is not PASS`);
}
exactStrings(result.deferred, profiles[profile].deferred, "deferred checks");

const severities = ["blocker", "critical", "high", "medium", "low"];
exactKeys(result.findings, severities, "findings");
for (const severity of severities) {
  if (!Number.isSafeInteger(result.findings[severity]) || result.findings[severity] !== 0) {
    fail(`${severity} findings must be exactly zero`);
  }
}

if (!Array.isArray(result.evidence) || result.evidence.length === 0) {
  fail("evidence must contain at least one executed command result");
}
for (const [index, evidence] of result.evidence.entries()) {
  exactKeys(evidence, ["command", "exit_code", "result"], `evidence[${index}]`);
  if (typeof evidence.command !== "string" || evidence.command.trim() === "") {
    fail(`evidence[${index}] command is empty`);
  }
  if (evidence.exit_code !== 0 || evidence.result !== "PASS") {
    fail(`evidence[${index}] is not a successful result`);
  }
}

process.stdout.write(
  `review-result-ok profile=${profile} source=${sourceSha} review=${reviewCommit} reviewer=${result.reviewer_id}\n`,
);
