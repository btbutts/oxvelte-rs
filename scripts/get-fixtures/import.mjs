#!/usr/bin/env node
// Mirrors discovery/config precedence in the pinned tests/utils/utils.ts.
// This exports raw cases, without generating or rewriting upstream expectations.
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse as parseYaml } from 'yaml';
import semver from 'semver';
import ts from 'typescript';
import { createRequire } from 'node:module';
import { builtinRules } from 'eslint/use-at-your-own-risk';
import { extractMembers, fetchGithubRelease, syncRuleDirs } from './github.mjs';

export const revision = '18339c886320151148568063c5801bf69cb51027';
export const environment = {
  svelte: '5.49.2', eslint: '10.9.1', typescript: '6.0.3',
  '@typescript-eslint/parser': '8.70.0', 'svelte-eslint-parser': '1.8.1',
};
const require = createRequire(import.meta.url);
if (require('eslint/package.json').version !== environment.eslint) throw new Error('Installed ESLint does not match the declared parity environment');
if (ts.version !== environment.typescript) throw new Error('Installed TypeScript does not match the declared parity environment');
const repository = 'https://github.com/sveltejs/eslint-plugin-svelte';
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
export const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
export const suffix = (name) => /\.svelte\.[jt]s$/u.test(name) ? name.slice(-10) : path.extname(name);
export const isInput = (name) => {
  const base = name.slice(0, name.length - suffix(name).length);
  return !name.startsWith('_') && (base.endsWith('input') || base.startsWith('+'));
};
export const companion = (input, replacement) => input.replace(/(input|\+.+)(?:\.[a-z]+)+$/u, replacement);
export function configCandidates(input) {
  return ['json', 'js', 'cjs'].flatMap((ext) => [
    companion(input, `config.${ext}`), path.posix.join(path.posix.dirname(input), `_config.${ext}`),
  ]);
}
export function ineligible(requirements, versions = environment) {
  return Object.entries(requirements).filter(([name, range]) => {
    if (name === 'FIXME') return false;
    if (!versions[name]) throw new Error(`No declared version for requirement ${name}`);
    if (!semver.validRange(range)) throw new Error(`Invalid version range: ${name} ${range}`);
    return !semver.satisfies(versions[name], range);
  }).map(([name, range]) => `${name} ${versions[name]} does not satisfy ${range}`);
}

// Inspect the rule's actual metadata and statically provable runtime guards.
// Never execute imported rule modules or infer capabilities from fixture names.
export function ruleMetadata(source, versions = environment, coreRules = builtinRules) {
  const file = ts.createSourceFile('rule.ts', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const declarations = new Map();
  const coreLoaders = new Set();
  const compilerVersions = new Set();
  const semverNames = new Set();
  for (const statement of file.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      const module = statement.moduleSpecifier.text;
      const clause = statement.importClause;
      if (module === 'semver' && clause?.name) semverNames.add(clause.name.text);
      if (clause?.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        for (const element of clause.namedBindings.elements) {
          const imported = element.propertyName?.text ?? element.name.text;
          if (module.endsWith('/eslint-core.js') && imported === 'getCoreRule') coreLoaders.add(element.name.text);
          if (module === 'svelte/compiler' && imported === 'VERSION') compilerVersions.add(element.name.text);
        }
      }
    }
    if (ts.isVariableStatement(statement) && (statement.declarationList.flags & ts.NodeFlags.Const)) for (const declaration of statement.declarationList.declarations) {
      if (ts.isIdentifier(declaration.name) && declaration.initializer) declarations.set(declaration.name.text, declaration.initializer);
    }
  }
  let rule;
  for (const statement of file.statements) {
    if (ts.isExportAssignment(statement) && ts.isCallExpression(statement.expression)) {
      const candidate = statement.expression.arguments[1];
      if (candidate && ts.isObjectLiteralExpression(candidate)) rule = candidate;
    }
  }
  if (!rule) return { fixable: false, ineligible: [] };
  const named = (object, name) => object.properties.find((property) => property.name && (ts.isIdentifier(property.name) || ts.isStringLiteral(property.name)) && property.name.text === name);
  const metadata = named(rule, 'meta');
  let fixable = false;
  if (metadata && ts.isPropertyAssignment(metadata) && ts.isObjectLiteralExpression(metadata.initializer)) {
    for (const property of metadata.initializer.properties) {
      if (ts.isSpreadAssignment(property) && ts.isPropertyAccessExpression(property.expression) && property.expression.name.text === 'meta' && ts.isIdentifier(property.expression.expression)) {
        const call = declarations.get(property.expression.expression.text);
        if (call && ts.isCallExpression(call) && ts.isIdentifier(call.expression) && coreLoaders.has(call.expression.text) && call.arguments.length === 1 && ts.isStringLiteral(call.arguments[0])) {
          const coreRule = coreRules.get(call.arguments[0].text);
          if (!coreRule) throw new Error(`Unknown inherited ESLint core rule ${call.arguments[0].text}`);
          fixable = ['code', 'whitespace'].includes(coreRule.meta?.fixable);
        }
      }
      if (ts.isPropertyAssignment(property) && (ts.isIdentifier(property.name) || ts.isStringLiteral(property.name)) && property.name.text === 'fixable') fixable = ts.isStringLiteral(property.initializer) && ['code', 'whitespace'].includes(property.initializer.text);
    }
  }
  const blocked = [];
  const create = named(rule, 'create');
  if (create && ts.isMethodDeclaration(create) && create.body) {
    for (const statement of create.body.statements) {
      if (!ts.isIfStatement(statement) || !ts.isPrefixUnaryExpression(statement.expression) || statement.expression.operator !== ts.SyntaxKind.ExclamationToken || !ts.isIdentifier(statement.expression.operand)) continue;
      const gateName = statement.expression.operand.text;
      if (create.parameters.some((parameter) => ts.isIdentifier(parameter.name) && parameter.name.text === gateName)) continue;
      if (create.body.statements.some((local) => ts.isVariableStatement(local) && local.declarationList.declarations.some((declaration) => ts.isIdentifier(declaration.name) && declaration.name.text === gateName))) continue;
      const gate = declarations.get(statement.expression.operand.text);
      const branch = statement.thenStatement;
      if (!ts.isBlock(branch) || branch.statements.length !== 1 || !ts.isReturnStatement(branch.statements[0])) continue;
      const returned = branch.statements[0].expression;
      if (!returned || !ts.isObjectLiteralExpression(returned) || returned.properties.length) continue;
      if (!gate || !ts.isCallExpression(gate) || !ts.isPropertyAccessExpression(gate.expression) || gate.expression.name.text !== 'satisfies' || !ts.isIdentifier(gate.expression.expression) || !semverNames.has(gate.expression.expression.text)) continue;
      const [version, range] = gate.arguments;
      if (!version || !ts.isIdentifier(version) || !compilerVersions.has(version.text) || !range || !ts.isStringLiteral(range) || gate.arguments.length !== 2) continue;
      blocked.push(...ineligible({ svelte: range.text }, versions).map((reason) => `Rule runtime gate: ${reason}`));
    }
  }
  return { fixable, ineligible: blocked };
}

export function buildManifest(files, versions = environment, coreRules = builtinRules) {
  const get = (name) => files.get(name)?.toString('utf8');
  const json = (name) => JSON.parse(get(name));
  const cases = [];
  const rules = new Set();
  const ruleMetadataCache = new Map();
  for (const [fullPath, bytes] of files) {
    if (!fullPath.startsWith('tests/fixtures/rules/')) continue;
    const id = fullPath.slice('tests/fixtures/rules/'.length);
    const parts = id.split('/');
    const kindIndex = parts.findIndex((part) => part === 'valid' || part === 'invalid');
    if (kindIndex < 1 || parts.slice(0, -1).some((part) => part.startsWith('_')) || !isInput(parts.at(-1))) continue;
    const rule = parts.slice(0, kindIndex).join('/');
    rules.add(rule);
    const source = bytes.toString('utf8');
    if (!source) throw new Error(`Empty input: ${id}`);
    const configFile = configCandidates(fullPath).find((name) => files.has(name));
    const executableConfig = configFile && !configFile.endsWith('.json');
    const config = configFile && !executableConfig ? json(configFile) : {};
    const requirementsFile = [companion(fullPath, 'requirements.json'), path.posix.join(path.posix.dirname(fullPath), '_requirements.json')].find((name) => files.has(name));
    const requirements = requirementsFile ? json(requirementsFile) : {};
    const sourceFile = `src/rules/${rule}.ts`;
    // Only Svelte rules expose fixability here. The adapted TS integration rule
    // has no source module, and is reported as unsupported by the Rust runner.
    if (!ruleMetadataCache.has(sourceFile)) ruleMetadataCache.set(sourceFile, ruleMetadata(get(sourceFile) ?? '', versions, coreRules));
    const metadata = ruleMetadataCache.get(sourceFile);
    const fixable = metadata.fixable;
    const errorsFile = companion(fullPath, 'errors.yaml');
    const outputFile = companion(fullPath, `output${suffix(fullPath)}`);
    let errors = [];
    let output = null;
    if (parts[kindIndex] === 'invalid') {
      if (!files.has(errorsFile)) throw new Error(`Missing expected diagnostics: ${errorsFile}`);
      errors = parseYaml(get(errorsFile));
      if (!Array.isArray(errors) || errors.some((e) => typeof e.message !== 'string' || !Number.isInteger(e.line) || !Number.isInteger(e.column))) {
        throw new Error(`Invalid expected diagnostics: ${errorsFile}`);
      }
      if (fixable) {
        if (!files.has(outputFile)) throw new Error(`Missing expected fix output: ${outputFile}`);
        output = get(outputFile) === source ? null : get(outputFile);
      }
    }
    cases.push({ id, rule, kind: parts[kindIndex], filename: fullPath, configFile: configFile ?? null,
      config, executableConfig: Boolean(executableConfig), requirements, ineligible: [...ineligible(requirements, versions), ...metadata.ineligible],
      fixable, errors, output });
  }
  cases.sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  if (!cases.length) throw new Error('No fixture cases discovered');
  return { schema: 1, repository, revision, version: '3.23.0', environment: versions,
    scope: 'Raw rule fixtures. Inline, processor, config and core integration tests are not included.',
    parserDefaults: { ecmaVersion: 'latest', sourceType: 'module', globals: 'browser',
      parserOptions: { project: 'tests/fixtures/rules/tsconfig.json', extraFileExtensions: ['.svelte'], parser: { ts: '@typescript-eslint/parser', js: 'espree' } } },
    ruleCount: rules.size, cases,
    files: Object.fromEntries([...files].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([name, bytes]) => [name, digest(bytes)])),
  };
}

// Job: refresh fixtures/parser/{legacy,modern}/ from the pinned sveltejs/svelte
// release tag. Additive only — see docs/research/test/pinned-rules.md.
async function importSvelteParserFixtures() {
  const tag = process.env.OXVELTE_FIXTURES_SVELTE_TAG;
  if (!tag) throw new Error('OXVELTE_FIXTURES_SVELTE_TAG is not set; run via `cargo get-svelte-fixtures`');
  const suites = [
    { prefix: 'packages/svelte/tests/parser-legacy/samples/', dest: 'fixtures/parser/legacy', label: 'Svelte Legacy Parser' },
    { prefix: 'packages/svelte/tests/parser-modern/samples/', dest: 'fixtures/parser/modern', label: 'Svelte Modern Parser' },
  ];
  const tmp = mkdtempSync(path.join(os.tmpdir(), 'oxvelte-fixtures-'));
  try {
    console.log(`Fetching sveltejs/svelte@${tag}...`);
    const archive = await fetchGithubRelease('sveltejs', 'svelte', tag, tmp);
    const extracted = path.join(tmp, 'extracted');
    extractMembers(archive, extracted, suites.map((suite) => suite.prefix));
    const untouched = [];
    for (const suite of suites) {
      const source = path.join(extracted, ...suite.prefix.split('/'));
      if (!existsSync(source)) throw new Error(`Archive has no ${suite.prefix} — upstream layout changed?`);
      untouched.push(...syncRuleDirs(source, path.join(repoRoot, suite.dest), suite.label));
    }
    if (untouched.length) {
      console.log('\nrules with no upstream candidate:');
      for (const label of untouched.sort()) console.log(`  ${label}`);
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

async function main() {
  const [job] = process.argv.slice(2);
  if (job === '--get-svelte-fixtures') return importSvelteParserFixtures();
  if (job === '--get-eslint-fixtures') {
    throw new Error('--get-eslint-fixtures is not implemented yet (stage 2 of docs/research/test/pinned-rules.md)');
  }
  throw new Error('Usage: import.mjs --get-svelte-fixtures | --get-eslint-fixtures');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  });
}
