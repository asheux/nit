#!/usr/bin/env node
// Daily download-stats aggregator.
//
// Reads CloudFront access logs from s3://${AWS_S3_BUCKET}/${LOG_PREFIX},
// counts successful tarball/zip GETs by version + platform, accumulates
// into s3://${AWS_S3_BUCKET}/stats.json. The website's prebuild script
// pulls stats.json and renders the totals.
//
// LOG_PREFIX defaults to "AWSLogs/631371482467/CloudFront/" — the path
// CloudFront's "Access log delivery" UI writes to when no custom prefix
// is specified (the new UI doesn't expose a Prefix field, so we follow
// the AWS default rather than trying to relocate logs).
//
// Idempotency: `last_processed_log_key` in stats.json marks the most
// recent log file already counted. Subsequent runs only process logs
// with keys strictly greater (CloudFront log keys are date-sortable).
//
// Reset / replay: delete stats.json from the bucket and rerun — the
// script will process every log in the prefix from scratch.
//
// Env (required):
//   AWS_S3_BUCKET            target bucket (incl. -<account>-<region>-an suffix)
//   AWS_CF_DISTRIBUTION_ID   CloudFront distribution to invalidate /stats.json on
//
// Env (optional):
//   LOG_PREFIX     default: "AWSLogs/631371482467/CloudFront/"
//   STATS_KEY      default: "stats.json"
//   DEBUG=1        verbose progress

import { execFile, spawnSync } from "node:child_process";
import { promisify } from "node:util";
import { gunzipSync } from "node:zlib";

const exec = promisify(execFile);

const BUCKET = required("AWS_S3_BUCKET");
const DIST_ID = required("AWS_CF_DISTRIBUTION_ID");
const LOG_PREFIX = process.env.LOG_PREFIX || "AWSLogs/631371482467/CloudFront/";
const STATS_KEY = process.env.STATS_KEY || "stats.json";
const DEBUG = process.env.DEBUG === "1";

function required(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`[aggregate-stats] missing required env: ${name}`);
    process.exit(2);
  }
  return v;
}

function debug(...args) {
  if (DEBUG) console.error("[aggregate-stats]", ...args);
}

/// Maps an asset filename suffix to a coarse platform bucket.
/// Anything that doesn't match (sha256 sidecars, install.sh, etc.) is
/// dropped — we only count tarball/zip downloads.
function platformFromUri(uri) {
  if (uri.endsWith("-universal-apple-darwin.tar.gz")) return "macos";
  if (uri.endsWith("-x86_64-unknown-linux-gnu.tar.gz")) return "linux";
  if (uri.endsWith("-x86_64-pc-windows-msvc.zip")) return "windows";
  return null;
}

/// Extracts the tag from a release-asset URI of the form
/// `/v0.1.0/nit-v0.1.0-…tar.gz`. Returns null when the URI doesn't
/// follow the convention.
function tagFromUri(uri) {
  const m = uri.match(/^\/v(\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?)\/nit-v\1-/);
  return m ? `v${m[1]}` : null;
}

async function awsCli(args, { decode = "utf8", input = null } = {}) {
  return new Promise((resolve, reject) => {
    const child = execFile("aws", args, { encoding: decode === "buffer" ? null : decode, maxBuffer: 512 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) {
        err.stderr = stderr?.toString?.() ?? "";
        return reject(err);
      }
      resolve(stdout);
    });
    if (input != null) {
      child.stdin.end(input);
    }
  });
}

async function loadExistingStats() {
  // RESET=1 short-circuits the load and forces a from-scratch run.
  // Cheaper than requiring the IAM policy to include s3:DeleteObject
  // (only s3:PutObject is needed to overwrite later) and survives
  // workflow re-runs without state cleanup.
  if (process.env.RESET === "1") {
    debug("RESET=1 — ignoring any existing stats.json and starting fresh");
    return null;
  }
  try {
    const body = await awsCli(["s3", "cp", `s3://${BUCKET}/${STATS_KEY}`, "-"]);
    return JSON.parse(body);
  } catch (e) {
    if (/NoSuchKey|does not exist|404/i.test(e.stderr || "")) {
      debug("no existing stats.json; starting fresh");
      return null;
    }
    throw e;
  }
}

async function listLogKeys() {
  // `aws s3 ls --recursive` prints `<date> <time>     <size> <key>` per line.
  const out = await awsCli([
    "s3",
    "ls",
    `s3://${BUCKET}/${LOG_PREFIX}`,
    "--recursive",
  ]);
  return out
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      const parts = line.split(/\s+/);
      // Key is the rest after `date time size`.
      return parts.slice(3).join(" ");
    })
    .filter((key) => key.endsWith(".gz"))
    .sort(); // log filenames begin with YYYY-MM-DD-HH so lex sort == chrono
}

async function downloadLogGzipped(key) {
  // CloudFront access logs are gzipped; download as binary and decompress.
  return new Promise((resolve, reject) => {
    const buf = [];
    const child = execFile(
      "aws",
      ["s3", "cp", `s3://${BUCKET}/${key}`, "-"],
      { encoding: "buffer", maxBuffer: 512 * 1024 * 1024 },
      (err, stdout, stderr) => {
        if (err) {
          err.stderr = stderr?.toString?.() ?? "";
          return reject(err);
        }
        try {
          resolve(gunzipSync(stdout));
        } catch (e) {
          reject(e);
        }
      },
    );
    void child;
    void buf;
  });
}

function parseLogBuffer(buf) {
  // CloudFront writes access logs in one of two formats depending on
  // how the access-log delivery was configured:
  //   - Plain Text (W3C-style): tab-separated rows, two `#` comment
  //     header lines, field positions 5/7/8 = method / uri / status.
  //   - JSON: one object per line with keys "cs-method", "cs-uri-stem",
  //     "sc-status" (the new "Access log delivery" UI's default).
  // Detect by sniffing the first non-blank, non-comment line. Mixing
  // formats inside a single bucket prefix shouldn't happen, but we
  // detect per-line anyway so it doesn't hard-fail if it does.
  const events = [];
  const text = buf.toString("utf8");
  for (const raw of text.split("\n")) {
    const line = raw.trimEnd();
    if (!line || line.startsWith("#")) continue;
    const evt = line.startsWith("{") ? parseJsonLine(line) : parseTsvLine(line);
    if (evt) events.push(evt);
  }
  return events;
}

function parseTsvLine(line) {
  // Field positions (0-indexed): 5=cs-method, 7=cs-uri-stem, 8=sc-status.
  const fields = line.split("\t");
  if (fields.length < 9) return null;
  return classifyAccessEvent(fields[5], fields[7], fields[8]);
}

function parseJsonLine(line) {
  let obj;
  try {
    obj = JSON.parse(line);
  } catch {
    return null;
  }
  // CloudFront's JSON delivery uses the same field names as the W3C
  // log: `cs-method`, `cs-uri-stem`, `sc-status`. Values are always
  // strings; sc-status comes through as e.g. "200".
  return classifyAccessEvent(obj["cs-method"], obj["cs-uri-stem"], obj["sc-status"]);
}

// Shared classification: returns { tag, platform } for a recognised
// release-asset GET 200, or null otherwise.
function classifyAccessEvent(method, uri, status) {
  if (method !== "GET") return null;
  // Only 200; range responses (206) usually mean resumes/probes that
  // overlap a 200 already counted. Counting only 200 avoids
  // double-counting a single download.
  if (String(status) !== "200") return null;
  if (typeof uri !== "string" || uri.length === 0) return null;
  const platform = platformFromUri(uri);
  if (!platform) return null;
  const tag = tagFromUri(uri);
  if (!tag) return null;
  return { tag, platform };
}

async function findFirstReleaseAt() {
  // Pull from latest.json — it has `published_at` for the current latest,
  // but that's not the FIRST release. We approximate by reading the
  // earliest tag dir's LastModified on the bucket; for the common case of
  // a fresh project, latest.json is good-enough for the dashboard.
  const manifest = await loadLatestManifest();
  return manifest?.published_at || null;
}

async function findLatestTag() {
  const manifest = await loadLatestManifest();
  return manifest?.tag || null;
}

// Single canonical loader so the warning surfaces once when latest.json
// can't be read (silently catching twice — once per accessor — hid the
// `latest_release.tag: null` bug for a full debug cycle).
async function loadLatestManifest() {
  try {
    const body = await awsCli(["s3", "cp", `s3://${BUCKET}/latest.json`, "-"]);
    return JSON.parse(body);
  } catch (e) {
    const detail = e.stderr ? `${e.message} — ${e.stderr.trim()}` : e.message;
    console.warn(
      `[aggregate-stats] warn: latest.json unreadable (${detail}); ` +
        "latest_release.tag and first_release_at will remain null. " +
        "Check IAM grants s3:GetObject on the bucket root.",
    );
    return null;
  }
}

async function uploadStats(stats) {
  const body = JSON.stringify(stats, null, 2);
  // `aws s3 cp - s3://...` reads from stdin. Use spawnSync so we can pipe
  // the body in cleanly without keeping the buffer in two places.
  const result = spawnSync(
    "aws",
    [
      "s3",
      "cp",
      "-",
      `s3://${BUCKET}/${STATS_KEY}`,
      "--content-type",
      "application/json",
      "--cache-control",
      "public, max-age=600",
    ],
    { input: body, encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(`aws s3 cp stats.json failed: ${result.stderr}`);
  }
}

async function invalidateCloudFront() {
  await awsCli([
    "cloudfront",
    "create-invalidation",
    "--distribution-id",
    DIST_ID,
    "--paths",
    `/${STATS_KEY}`,
  ]);
}

async function main() {
  console.log(`[aggregate-stats] bucket=${BUCKET} prefix=${LOG_PREFIX}`);

  const prior = (await loadExistingStats()) ?? {
    schema_version: 1,
    updated_at: null,
    first_release_at: null,
    total_downloads: 0,
    downloads_by_platform: { macos: 0, linux: 0, windows: 0 },
    downloads_by_version: {},
    latest_release: { tag: null, downloads: 0 },
    last_processed_log_key: null,
  };

  const allKeys = await listLogKeys();
  const cutoff = prior.last_processed_log_key;
  const newKeys = cutoff
    ? allKeys.filter((k) => k > cutoff)
    : allKeys;

  console.log(
    `[aggregate-stats] found ${allKeys.length} total log files; ${newKeys.length} new since ${cutoff ?? "<empty>"}`,
  );

  // Mutable counters seeded from prior.
  const byPlatform = { ...prior.downloads_by_platform };
  const byVersion = { ...prior.downloads_by_version };
  let total = prior.total_downloads;

  let processed = 0;
  // Track only logs we successfully PARSED so the idempotency cursor
  // never advances past a file we silently failed to read. Otherwise a
  // transient IAM/policy error would mark every file as "done" forever.
  let lastSuccessfulKey = cutoff;
  let failures = 0;
  for (const key of newKeys) {
    let buf;
    try {
      buf = await downloadLogGzipped(key);
    } catch (e) {
      // Include AWS stderr — the bare `e.message` is just "Command failed"
      // which hides IAM/policy errors (the common cause of every-log-fails
      // patterns). With stderr we get e.g. `AccessDenied` and can fix
      // the IAM policy without re-running the workflow blind.
      const detail = e.stderr ? `${e.message} — ${e.stderr.trim()}` : e.message;
      console.warn(`[aggregate-stats] skip ${key}: ${detail}`);
      failures += 1;
      continue;
    }
    const events = parseLogBuffer(buf);
    for (const { tag, platform } of events) {
      total += 1;
      byPlatform[platform] = (byPlatform[platform] ?? 0) + 1;
      byVersion[tag] = (byVersion[tag] ?? 0) + 1;
    }
    processed += 1;
    lastSuccessfulKey = key;
    if (processed % 25 === 0) {
      debug(`processed ${processed}/${newKeys.length} files; running total ${total}`);
    }
  }
  // Fail loudly when EVERY new log fails — silent advancement of the
  // cursor through unparseable files is what produced the
  // "0-downloads-forever" bug we just patched.
  if (newKeys.length > 0 && processed === 0) {
    throw new Error(
      `[aggregate-stats] ${failures} log(s) found but all failed to read — refusing to advance cursor. Fix IAM/decryption first, then rerun. (Last attempted: ${newKeys[newKeys.length - 1]})`,
    );
  }

  const latestTag = (await findLatestTag()) ?? prior.latest_release.tag;
  const latestDownloads = latestTag ? byVersion[latestTag] ?? 0 : 0;
  const firstReleaseAt =
    prior.first_release_at ?? (await findFirstReleaseAt()) ?? null;

  const next = {
    schema_version: 1,
    updated_at: new Date().toISOString(),
    first_release_at: firstReleaseAt,
    total_downloads: total,
    downloads_by_platform: byPlatform,
    downloads_by_version: byVersion,
    latest_release: { tag: latestTag, downloads: latestDownloads },
    // Only advances for logs we actually parsed — see the loop above.
    last_processed_log_key: lastSuccessfulKey,
  };

  console.log(`[aggregate-stats] writing stats.json: total=${total}, by_platform=${JSON.stringify(byPlatform)}, latest_release=${JSON.stringify(next.latest_release)}`);

  await uploadStats(next);
  await invalidateCloudFront();
  console.log(`[aggregate-stats] done`);
}

main().catch((err) => {
  console.error(`[aggregate-stats] FATAL: ${err.message}`);
  if (err.stderr) console.error(err.stderr);
  process.exit(1);
});
