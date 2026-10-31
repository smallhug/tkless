#!/usr/bin/env node
// Static checks for the DeepSeek Harness bundle. Runs in CI without the DSH
// installation: it only inspects the committed files.

const fs = require('node:fs')
const path = require('node:path')
const { pathToFileURL } = require('node:url')

const root = path.join(__dirname, '..')
const failures = []

function fail(message) {
  failures.push(message)
}

function readJson(relative) {
  try {
    return JSON.parse(fs.readFileSync(path.join(root, relative), 'utf8'))
  } catch (error) {
    fail(`${relative}: ${error.message}`)
    return undefined
  }
}

const pkg = readJson('package.json')
if (pkg) {
  if (pkg.name !== 'tkless-dsh') fail(`package.json name is ${pkg.name}`)
  if (pkg.private !== true) fail('package.json must stay private')
  if (pkg.type !== 'module') fail('package.json must declare "type": "module"')
  if (pkg.dsh?.bundle?.patch !== './cordis.patch.yml') fail('package.json dsh.bundle.patch is missing')
  if (!pkg.engines?.dsh) fail('package.json must pin engines.dsh to the supported DSH version')
  for (const required of ['cordis.patch.yml', 'dsh/index.js', 'locale/*.json', 'assets/logo.svg']) {
    if (!Array.isArray(pkg.files) || !pkg.files.includes(required)) {
      fail(`package.json files must include ${required}`)
    }
  }
  if (pkg.exports?.['./icon'] !== './assets/logo.svg') fail('package.json must export ./icon')
}

for (const locale of ['en', 'zh']) {
  const data = readJson(`locale/${locale}.json`)
  if (!data?.meta?.title || !data?.meta?.description) {
    fail(`locale/${locale}.json must define meta.title and meta.description`)
  }
}

try {
  const icon = fs.statSync(path.join(root, 'assets/logo.svg'))
  if (icon.size > 256 * 1024) fail(`assets/logo.svg exceeds the 256 KiB icon limit (${icon.size} bytes)`)
} catch {
  fail('assets/logo.svg is missing')
}

const upstream = readJson('dsh/upstream.json')
if (upstream) {
  if (pkg?.engines?.dsh !== upstream.dshVersion) {
    fail(`engines.dsh (${pkg?.engines?.dsh}) does not match dsh/upstream.json (${upstream.dshVersion})`)
  }
  for (const id of ['standard', 'ptc', 'cordis']) {
    if (typeof upstream.presets?.[id] !== 'string') fail(`dsh/upstream.json is missing the ${id} hash`)
  }
}

let patch = ''
try {
  patch = fs.readFileSync(path.join(root, 'cordis.patch.yml'), 'utf8')
} catch {
  fail('cordis.patch.yml is missing')
}

if (patch) {
  if (!/- id: tkless-dsh[\s\S]*?name: '\.\/dsh\/index\.js'/.test(patch)) {
    fail('cordis.patch.yml must insert the tkless-dsh bridge')
  }
  const overrides = [...patch.matchAll(/^- id: (preset-[a-z]+)$/gm)].map((match) => match[1])
  for (const id of ['preset-standard', 'preset-ptc', 'preset-cordis']) {
    if (!overrides.includes(id)) fail(`cordis.patch.yml is missing the ${id} override`)
  }
  if (overrides.length !== 3) fail(`expected 3 preset overrides, found ${overrides.length}`)
  const disabled = [...patch.matchAll(/^\s+search: false$/gm)].length
  if (disabled !== 3) fail(`expected 3 "search: false" entries, found ${disabled}`)
  const fetches = [...patch.matchAll(/^\s+fetch: true$/gm)].length
  if (fetches !== 3) fail(`expected 3 preserved "fetch: true" entries, found ${fetches}`)
  if (/^\s+search: true$/m.test(patch)) fail('cordis.patch.yml must not enable web_search')
}

const cargo = fs.existsSync(path.join(root, 'Cargo.toml'))
  ? fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8')
  : ''
const cargoVersion = /^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1]
if (cargoVersion && pkg?.version !== cargoVersion) {
  fail(`package.json version (${pkg?.version}) does not match the Cargo workspace (${cargoVersion})`)
}

const syntaxTarget = path.join(root, 'dsh/index.js')

let bridge = ''
try {
  bridge = fs.readFileSync(syntaxTarget, 'utf8')
} catch {
  fail('dsh/index.js is missing')
}

if (bridge) {
  for (const needle of [
    "server/discover",
    'io.modelcontextprotocol/protocolVersion',
    'io.modelcontextprotocol/clientCapabilities',
    '2026-07-28',
    'structuredContent',
    'notifications/progress',
    'progressToken',
  ]) {
    if (!bridge.includes(needle)) fail(`dsh/index.js is missing the 2026-07-28 requirement: ${needle}`)
  }
  if (/request\(\s*['"]initialize['"]/.test(bridge)) {
    fail('dsh/index.js must not use the removed initialize handshake')
  }
  if (/['"]2025-11-25['"]/.test(bridge)) {
    fail('dsh/index.js still references the retired 2025-11-25 protocol')
  }
  if (!/rawName === ['"]activate_tools['"][\s\S]{0,240}await this\.refreshTools\(\)/.test(bridge)) {
    fail('dsh/index.js must refresh tools before returning from activate_tools')
  }
}

function finish() {
  if (failures.length > 0) {
    for (const failure of failures) console.error(`check-dsh-bundle: ${failure}`)
    process.exit(1)
  }
  console.log('check-dsh-bundle: ok')
}

import(pathToFileURL(syntaxTarget).href).then(finish, (error) => {
  fail(`dsh/index.js failed to load: ${error?.message ?? error}`)
  finish()
})
