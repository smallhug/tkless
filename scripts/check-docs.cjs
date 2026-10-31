// Development-only check: Markdown examples/links and the executable MCP catalog.
// Run after cargo build: node scripts/check-docs.cjs [path/to/tkless]
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

process.chdir(path.resolve(__dirname, '..'));
const executable = path.resolve(process.argv[2] ||
  `target/debug/tkless${process.platform === 'win32' ? '.exe' : ''}`);
const documents = ['README.md', 'RELEASE_NOTES.md', 'CHANGELOG.md',
  ...fs.readdirSync('doc').filter(name => name.endsWith('.md')).map(name => `doc/${name}`)];
const errors = [];
let jsonBlocks = 0;
let localLinks = 0;

for (const file of documents) {
  const text = fs.readFileSync(file, 'utf8');
  let open = null;
  let body = [];
  for (const line of text.split(/\r?\n/)) {
    const fence = line.match(/^\s*(`{3,}|~{3,})(.*)$/);
    if (!fence) {
      if (open) body.push(line);
      continue;
    }
    if (!open) {
      open = { marker: fence[1][0], length: fence[1].length, language: fence[2].trim() };
      body = [];
    } else if (fence[1][0] === open.marker && fence[1].length >= open.length && !fence[2].trim()) {
      if (['json', 'jsonl'].includes(open.language)) {
        try {
          if (open.language === 'json') JSON.parse(body.join('\n'));
          else body.filter(line => line.trim()).forEach(line => JSON.parse(line));
          jsonBlocks++;
        } catch (error) {
          errors.push(`${file}: invalid ${open.language}: ${error.message}`);
        }
      }
      open = null;
    } else body.push(line);
  }
  if (open) errors.push(`${file}: unclosed code fence`);
  const prose = text.replace(/^\s*```[\s\S]*?^\s*```\s*$/gm, '');
  for (const match of prose.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    const raw = match[1].split(/\s+"/)[0].replace(/^<|>$/g, '');
    if (/^(?:https?:|mailto:|#)/.test(raw)) continue;
    const relative = decodeURIComponent(raw.split('#')[0]);
    if (!relative) continue;
    localLinks++;
    if (!fs.existsSync(path.resolve(path.dirname(file), relative))) {
      errors.push(`${file}: missing link ${relative}`);
    }
  }
}

const catalogs = [];
const meta = {
  'io.modelcontextprotocol/protocolVersion': '2026-07-28',
  'io.modelcontextprotocol/clientCapabilities': {},
};
for (const args of [['mcp', '--no-cache'], ['mcp', '--dynamic', '--no-cache']]) {
  const requests = [
    { jsonrpc: '2.0', id: 1, method: 'server/discover', params: { _meta: meta } },
    { jsonrpc: '2.0', id: 2, method: 'tools/list', params: { _meta: meta } },
  ];
  const input = requests.map((request) => JSON.stringify(request)).join('\n') + '\n';
  const result = spawnSync(executable, args, { input, encoding: 'utf8', timeout: 10000 });
  try {
    if (result.status !== 0) throw new Error(result.error?.message || result.stderr);
    const responses = result.stdout.trim().split(/\r?\n/).map((line) => JSON.parse(line));
    const discover = responses.find((response) => response.id === 1);
    if (!discover?.result?.supportedVersions?.includes('2026-07-28')) {
      throw new Error('server/discover does not advertise 2026-07-28');
    }
    const catalog = responses.find((response) => response.id === 2);
    catalogs.push(catalog.result.tools);
  } catch (error) {
    errors.push(`MCP catalog check failed: ${error.message}`);
  }
}
if (catalogs.length === 2) {
  if (catalogs[0].length !== 31 || catalogs[1].length !== 6) errors.push('Unexpected MCP tool counts');
  for (const name of ['collect_pages', 'sync_collection', 'browse_page', 'list_wechat_articles',
    'jobs_start', 'jobs_status', 'jobs_cancel', 'jobs_list']) {
    if (!catalogs[0].some(tool => tool.name === name)) errors.push(`Missing workflow tool: ${name}`);
  }
  const search = catalogs[0].find(tool => tool.name === 'web_search')?.inputSchema.properties;
  if (!search || search.limit.default !== 10 || search.limit.maximum !== 20 ||
      search.auto_cdp.default !== false || search.engine || search.yandex_region) {
    errors.push('Bing search contract differs from documentation');
  }
}
console.log(JSON.stringify({ documents: documents.length, jsonBlocks, localLinks,
  fullTools: catalogs[0]?.length, dynamicTools: catalogs[1]?.length, errors }, null, 2));
if (errors.length) process.exitCode = 1;
