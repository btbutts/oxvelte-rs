// Shared GitHub release-archive helpers for the fixture importers.
// Generic by design: no repo names, fixture paths, or rule knowledge live
// here — callers pass those in. Uses only node: builtins so it runs
// identically under node and bun.
import { cpSync, createWriteStream, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { gunzipSync } from 'node:zlib';

// Download https://github.com/<owner>/<repo>/archive/refs/tags/<repo>@<tag>.tar.gz
// into destDir and return the archive path.
export async function fetchGithubRelease(owner, repo, tag, destDir) {
  const url = `https://github.com/${owner}/${repo}/archive/refs/tags/${repo}@${tag}.tar.gz`;
  const destination = path.join(destDir, `${repo}-${tag}.tar.gz`);
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok || !response.body) throw new Error(`Download failed (${response.status}): ${url}`);
  await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
  return destination;
}

const headerString = (header, start, length) =>
  header.subarray(start, start + length).toString('utf8').replace(/\0.*$/su, '').trim();

// pax extended-header records are "<length> <key>=<value>\n"; we only need path.
const paxPath = (body) => {
  const match = /(?:^|\n)\d+ path=([^\n]+)/u.exec(body.toString('utf8'));
  return match ? match[1] : null;
};

// Walk a gzipped tar buffer, yielding { name, type, body } per entry.
// Handles ustar prefixes plus GNU ('L') and pax ('x') long-name records.
function* tarEntries(buffer) {
  let offset = 0;
  let longName = null;
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) return;
    const name = headerString(header, 0, 100);
    const size = parseInt(headerString(header, 124, 12), 8);
    const type = String.fromCharCode(header[156]);
    const prefix = headerString(header, 345, 155);
    offset += 512;
    const body = buffer.subarray(offset, offset + size);
    offset += Math.ceil(size / 512) * 512;
    if (type === 'L') { longName = body.toString('utf8').replace(/\0.*$/su, ''); continue; }
    if (type === 'x') { longName = paxPath(body) ?? longName; continue; }
    if (type === 'g') continue;
    const fullName = longName ?? (prefix ? `${prefix}/${name}` : name);
    longName = null;
    yield { name: fullName, type, body };
  }
}

// Extract regular files whose path (after stripping the archive's top-level
// "<repo>-<ref>/" directory) starts with one of `prefixes` into destDir,
// preserving the remaining directory structure. Returns the file count.
export function extractMembers(archivePath, destDir, prefixes) {
  const buffer = gunzipSync(readFileSync(archivePath));
  const matches = (relative) => prefixes.some((prefix) =>
    prefix.endsWith('/') ? relative.startsWith(prefix) : relative === prefix || relative.startsWith(`${prefix}/`));
  let count = 0;
  for (const entry of tarEntries(buffer)) {
    if (entry.type !== '0' && entry.type !== '\0') continue;
    const slash = entry.name.indexOf('/');
    if (slash < 0) continue;
    const relative = entry.name.slice(slash + 1);
    if (!matches(relative)) continue;
    const destination = path.join(destDir, ...relative.split('/'));
    mkdirSync(path.dirname(destination), { recursive: true });
    writeFileSync(destination, entry.body);
    count += 1;
  }
  if (!count) throw new Error(`No archive members matched: ${prefixes.join(', ')}`);
  return count;
}

export const listFiles = (dir, relative = '') =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const name = relative ? `${relative}/${entry.name}` : entry.name;
    return entry.isDirectory() ? listFiles(path.join(dir, entry.name), name) : [name];
  }).sort();

function dirsEqual(a, b) {
  const filesA = listFiles(a);
  const filesB = listFiles(b);
  if (filesA.length !== filesB.length || filesA.some((file, index) => file !== filesB[index])) return false;
  return filesA.every((file) => readFileSync(path.join(a, file)).equals(readFileSync(path.join(b, file))));
}

// Sync one named rule directory per child of sourceDir into destDir, printing
// "<labelPrefix> - <rule>: CREATE|UPDATE|SKIP" per rule. UPDATE replaces the
// whole rule directory so it ends up byte-identical to the source. Additive
// only: existing destination rules with no source counterpart are left alone
// and returned as labels for the caller's "no upstream candidate" report.
export function syncRuleDirs(sourceDir, destDir, labelPrefix) {
  const sourceRules = readdirSync(sourceDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
  for (const rule of sourceRules) {
    const from = path.join(sourceDir, rule);
    const to = path.join(destDir, rule);
    const label = `${labelPrefix} - ${rule}`;
    if (!existsSync(to)) {
      mkdirSync(path.dirname(to), { recursive: true });
      cpSync(from, to, { recursive: true });
      console.log(`${label}: CREATE`);
    } else if (dirsEqual(from, to)) {
      console.log(`${label}: SKIP`);
    } else {
      rmSync(to, { recursive: true, force: true });
      cpSync(from, to, { recursive: true });
      console.log(`${label}: UPDATE`);
    }
  }
  if (!existsSync(destDir)) return [];
  return readdirSync(destDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && !sourceRules.includes(entry.name))
    .map((entry) => `${labelPrefix} - ${entry.name}`);
}
