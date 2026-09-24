// Runs the commit-analyzer and release-notes-generator plugins from
// .releaserc.json over a fixed set of commits and fails unless they pick the
// expected release type and render every expected note section.
//
// Both plugins load their preset (conventional-changelog-conventionalcommits)
// by name at runtime, and a preset built for a different plugin major loads
// without error but renders no commits: v0.16.0 through v0.18.2 shipped with
// empty release notes this way. This check makes that failure loud.

import { readFileSync } from 'node:fs';

const REPOSITORY_URL = 'https://github.com/camercu/relentless';
const ANALYZER = '@semantic-release/commit-analyzer';
const NOTES = '@semantic-release/release-notes-generator';

const pluginConfig = (name) => {
  const entry = JSON.parse(readFileSync('.releaserc.json', 'utf8')).plugins.find(
    (plugin) => (Array.isArray(plugin) ? plugin[0] : plugin) === name,
  );
  if (!entry) throw new Error(`.releaserc.json has no ${name} plugin`);
  return Array.isArray(entry) ? entry[1] : {};
};

const commit = (message, index) => ({
  hash: index.toString(16).padStart(40, '0'),
  message,
});

const context = (commits) => ({
  cwd: process.cwd(),
  env: process.env,
  options: { repositoryUrl: REPOSITORY_URL },
  commits: commits.map(commit),
  lastRelease: { gitTag: 'v1.0.0', version: '1.0.0' },
  nextRelease: { gitTag: 'v1.1.0', version: '1.1.0' },
  logger: { log() {}, error: console.error },
});

const { analyzeCommits } = await import(ANALYZER);
const { generateNotes } = await import(NOTES);

const failures = [];

const releaseCases = [
  { commits: ['chore: tidy'], expected: null },
  { commits: ['fix(ci): pin a tool'], expected: null },
  { commits: ['fix(ci)!: drop a tool'], expected: 'minor' },
  {
    commits: ['fix(ci): drop a tool\n\nBREAKING CHANGE: callers install it.'],
    expected: 'minor',
  },
  { commits: ['fix: stop a panic'], expected: 'patch' },
  { commits: ['fix(engine): stop a panic'], expected: 'patch' },
  { commits: ['feat(wait): add a strategy'], expected: 'minor' },
  { commits: ['refactor(api)!: rename a method'], expected: 'minor' },
  {
    commits: ['refactor(api): rename a method\n\nBREAKING CHANGE: callers rename.'],
    expected: 'minor',
  },
];
for (const { commits, expected } of releaseCases) {
  const actual = await analyzeCommits(pluginConfig(ANALYZER), context(commits));
  if (actual !== expected) {
    failures.push(`release type for ${JSON.stringify(commits)}: got ${actual}, want ${expected}`);
  }
}

const notes = await generateNotes(
  pluginConfig(NOTES),
  context([
    'feat(wait): add a strategy',
    'fix(engine): stop a panic',
    'perf(jitter): skip a draw',
    'refactor(api)!: rename a method',
    'docs: hidden from notes',
  ]),
);
const expectedNotes = [
  '### ⚠ BREAKING CHANGES',
  '### Features',
  '* **wait:** add a strategy',
  '### Bug Fixes',
  '* **engine:** stop a panic',
  '### Performance Improvements',
  '* **api:** rename a method',
];
for (const line of expectedNotes) {
  if (!notes.includes(line)) failures.push(`release notes lack ${JSON.stringify(line)}`);
}
if (notes.includes('hidden from notes')) failures.push('release notes show a hidden docs commit');

if (failures.length > 0) {
  console.error('Release config check failed:');
  for (const failure of failures) console.error(`  - ${failure}`);
  console.error('\nRendered notes:\n' + notes);
  process.exit(1);
}
console.log('Release config check passed.');
