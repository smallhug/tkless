// TkLess DeepSeek Harness bridge.
//
// Responsibilities:
//   1. Provision the `tkless` binary into this package's `bin/` directory,
//      verifying the release SHA-256 manifest before use.
//   2. Start `tkless mcp --dynamic` over stdio and expose its tools through
//      the Harness tool registry as `mcp__tkless__<tool>`.
//   3. Deny `web_search` for sessions that started before the preset override
//      took effect, and expose status/retry tools so the state is observable.
//
// The binary lives inside the installed bundle package: `pnpm remove` deletes
// the package directory, which removes the binary with it.

import { createHash } from 'node:crypto'
import { execFile, spawn } from 'node:child_process'
import { promises as fs } from 'node:fs'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const PACKAGE_DIR = path.dirname(HERE)
const MAX_BINARY_BYTES = 200 * 1024 * 1024
const MIN_BINARY_BYTES = 1024 * 1024
const CONNECT_TIMEOUT_MS = 30_000
const DOWNLOAD_TIMEOUT_MS = 180_000
const CALL_TIMEOUT_MS = 600_000
const DEFAULT_ARGS = ['mcp', '--dynamic']
const DEFAULT_PREFIX = 'mcp__tkless__'
const BINARY_NAME = process.platform === 'win32' ? 'tkless.exe' : 'tkless'

function log(...args) {
  console.error('[tkless-dsh]', ...args)
}

function errorSummary(error) {
  if (!error) return 'unknown error'
  const message = error.message ?? String(error)
  const code = typeof error.code === 'number' ? ` (code ${error.code})` : ''
  const data = error.data === undefined ? '' : ` ${JSON.stringify(error.data)}`
  return `${message}${code}${data}`
}

function binaryPathInPackage() {
  return path.join(PACKAGE_DIR, 'bin', BINARY_NAME)
}

function platformAsset() {
  const { platform, arch } = process
  if (platform === 'win32' && arch === 'x64') return 'tkless-windows-amd64.exe'
  if (platform === 'linux' && arch === 'x64') return 'tkless-linux-amd64'
  if (platform === 'linux' && arch === 'arm64') return 'tkless-linux-arm64'
  return undefined
}

async function fileBytes(file) {
  try {
    const stat = await fs.stat(file)
    return stat.isFile() ? stat.size : -1
  } catch {
    return -1
  }
}

async function packageVersion() {
  const raw = await fs.readFile(path.join(PACKAGE_DIR, 'package.json'), 'utf8')
  return JSON.parse(raw).version
}

function releaseBase(options, version) {
  if (typeof options.downloadBaseUrl === 'string' && options.downloadBaseUrl.length > 0) {
    return options.downloadBaseUrl.replace(/\/+$/, '')
  }
  const tag = typeof options.releaseTag === 'string' && options.releaseTag.length > 0
    ? options.releaseTag
    : `v${version}`
  return `https://github.com/smallhug/tkless/releases/download/${tag}`
}

async function fetchOk(url, signal) {
  const response = await fetch(url, { redirect: 'follow', signal })
  if (!response.ok) {
    throw new Error(`download failed with HTTP ${response.status}: ${url}`)
  }
  return response
}

function parseChecksums(text) {
  const map = new Map()
  for (const line of text.split(/\r?\n/)) {
    const match = /^([a-fA-F0-9]{64})\s+\*?(.+?)\s*$/.exec(line.trim())
    if (match) map.set(match[2], match[1].toLowerCase())
  }
  return map
}

async function downloadBinary(options, version) {
  const asset = platformAsset()
  if (!asset) {
    throw new Error(
      `automatic download is not available for ${process.platform}/${process.arch}; ` +
        'install tkless yourself and set binaryPath in the plugin config',
    )
  }

  const base = releaseBase(options, version)
  const signal = AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS)
  log(`downloading ${asset} from ${base}`)

  const sumsResponse = await fetchOk(`${base}/SHA256SUMS`, signal)
  const expected = parseChecksums(await sumsResponse.text()).get(asset)
  if (!expected) {
    throw new Error(`SHA256SUMS from ${base} does not list ${asset}; refusing to install`)
  }

  const binaryResponse = await fetchOk(`${base}/${asset}`, signal)
  const declared = Number(binaryResponse.headers.get('content-length') ?? 0)
  if (declared > MAX_BINARY_BYTES) {
    throw new Error(`refusing to download ${asset}: declared size ${declared} exceeds limit`)
  }
  const bytes = Buffer.from(await binaryResponse.arrayBuffer())
  if (bytes.length > MAX_BINARY_BYTES) {
    throw new Error(`refusing to install ${asset}: downloaded ${bytes.length} bytes exceeds limit`)
  }

  const actual = createHash('sha256').update(bytes).digest('hex')
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${asset}: expected ${expected}, got ${actual}`)
  }

  const destination = binaryPathInPackage()
  await fs.mkdir(path.dirname(destination), { recursive: true })
  const temporary = `${destination}.tmp-${process.pid}`
  await fs.writeFile(temporary, bytes, { mode: 0o755 })
  await fs.rm(destination, { force: true })
  await fs.rename(temporary, destination)
  if (process.platform !== 'win32') await fs.chmod(destination, 0o755)
  log(`installed ${asset} (${bytes.length} bytes) at ${destination}`)
  return destination
}

async function ensureBinary(options, version) {
  const configured = options.binaryPath || process.env.TKLESS_DSH_BINARY
  if (typeof configured === 'string' && configured.trim().length > 0) {
    const resolved = path.resolve(configured)
    const size = await fileBytes(resolved)
    if (size <= 0) throw new Error(`configured tkless binary is missing or unreadable: ${resolved}`)
    return resolved
  }

  const bundled = binaryPathInPackage()
  if ((await fileBytes(bundled)) >= MIN_BINARY_BYTES) return bundled

  if (options.autoDownload === false) {
    throw new Error(
      'tkless binary is not installed; enable autoDownload, set binaryPath, or install tkless on PATH',
    )
  }
  return downloadBinary(options, version)
}

function probeBinary(binary) {
  return new Promise((resolve, reject) => {
    execFile(binary, ['--version'], { timeout: 15_000, windowsHide: true }, (error, stdout, stderr) => {
      if (error) {
        reject(new Error(`downloaded tkless failed its version probe: ${stderr || error.message}`))
        return
      }
      resolve(String(stdout).trim())
    })
  })
}

function normalizeInputSchema(schema) {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) {
    return { type: 'object', properties: {} }
  }
  const copy = { ...schema }
  if (copy.type === undefined) copy.type = 'object'
  return copy
}

function renderResultText(result) {
  const parts = []
  for (const block of result?.content ?? []) {
    if (block?.type === 'text' && typeof block.text === 'string') {
      parts.push(block.text)
    } else if (block?.type === 'image') {
      parts.push('[image result omitted by tkless-dsh]')
    } else if (block?.type === 'resource' && block.resource?.uri) {
      parts.push(`[resource] ${block.resource.uri}`)
    } else {
      parts.push(JSON.stringify(block))
    }
  }
  if (result?.isError) {
    throw new Error(parts.join('\n') || 'tkless tool reported an error')
  }
  return parts.join('\n')
}

class McpClient {
  constructor(child, onNotification, options = {}) {
    this.child = child
    this.onNotification = onNotification
    this.pending = new Map()
    this.progressHandlers = new Map()
    this.nextId = 1
    this.exited = false
    this.protocolVersion = options.protocolVersion ?? '2026-07-28'
    this.clientInfo = options.clientInfo ?? { name: 'tkless-dsh', version: 'unknown' }
    this.reader = createInterface({ input: child.stdout })

    this.reader.on('line', (line) => this.handleLine(line))
    child.stdin.on('error', (error) => log('tkless stdin error:', error?.message ?? error))
    child.stderr.setEncoding('utf8')
    child.stderr.on('data', (chunk) => {
      for (const line of String(chunk).split(/\r?\n/)) {
        if (line.trim().length > 0) log('mcp:', line)
      }
    })
    child.on('error', (error) => this.failAll(error))
    child.on('exit', (code, signal) => {
      this.exited = true
      this.failAll(new Error(`tkless mcp exited (${code ?? signal ?? 'unknown'})`))
      try {
        this.reader.close()
      } catch {
        // already closed
      }
    })
  }

  handleLine(line) {
    const trimmed = line.trim()
    if (trimmed.length === 0) return
    let message
    try {
      message = JSON.parse(trimmed)
    } catch {
      log('ignoring non-JSON stdout line:', trimmed.slice(0, 200))
      return
    }
    if (message.id !== undefined && (message.result !== undefined || message.error !== undefined)) {
      const pending = this.pending.get(message.id)
      if (!pending) return
      this.pending.delete(message.id)
      pending.cleanup()
      if (message.error) {
        const error = new Error(message.error.message || JSON.stringify(message.error))
        error.code = message.error.code
        error.data = message.error.data
        pending.reject(error)
      } else {
        pending.resolve(message.result)
      }
      return
    }
    if (message.method === 'notifications/progress') {
      const token = message.params?.progressToken
      const handler = token === undefined ? undefined : this.progressHandlers.get(token)
      if (handler) {
        try {
          handler(message.params)
        } catch (error) {
          log('progress handler failed:', errorSummary(error))
        }
      }
    }
    if (typeof message.method === 'string') this.onNotification?.(message)
  }

  request(method, params, { timeoutMs = CONNECT_TIMEOUT_MS, signal, onProgress } = {}) {
    if (this.exited) return Promise.reject(new Error('tkless mcp is not running'))
    const id = this.nextId++
    const paramsWithMeta = {
      ...(params && typeof params === 'object' && !Array.isArray(params) ? params : {}),
    }
    const meta = {
      ...(paramsWithMeta._meta && typeof paramsWithMeta._meta === 'object'
        ? paramsWithMeta._meta
        : {}),
      'io.modelcontextprotocol/protocolVersion': this.protocolVersion,
      'io.modelcontextprotocol/clientInfo': this.clientInfo,
      'io.modelcontextprotocol/clientCapabilities': {},
    }
    let progressToken
    if (typeof onProgress === 'function') {
      progressToken = `tkless-dsh-${id}`
      meta.progressToken = progressToken
    }
    paramsWithMeta._meta = meta
    const payload = { jsonrpc: '2.0', id, method, params: paramsWithMeta }

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const entry = this.pending.get(id)
        this.pending.delete(id)
        entry?.cleanup()
        reject(new Error(`tkless ${method} timed out after ${timeoutMs} ms`))
      }, timeoutMs)

      const onAbort = () => {
        const entry = this.pending.get(id)
        this.pending.delete(id)
        entry?.cleanup()
        this.notify('notifications/cancelled', { requestId: id, reason: 'aborted' })
        reject(new Error('tkless call aborted'))
      }
      if (signal) {
        if (signal.aborted) {
          onAbort()
          return
        }
        signal.addEventListener('abort', onAbort, { once: true })
      }

      this.pending.set(id, {
        resolve,
        reject,
        cleanup: () => {
          clearTimeout(timer)
          signal?.removeEventListener('abort', onAbort)
          if (progressToken) this.progressHandlers.delete(progressToken)
        },
      })
      if (progressToken) this.progressHandlers.set(progressToken, onProgress)
      this.write(payload)
    })
  }

  notify(method, params) {
    if (this.exited) return
    const payload = { jsonrpc: '2.0', method }
    if (params !== undefined) payload.params = params
    this.write(payload)
  }

  write(payload) {
    if (this.exited) return
    try {
      this.child.stdin.write(`${JSON.stringify(payload)}\n`)
    } catch (error) {
      log('failed to write to tkless stdin:', error?.message ?? error)
    }
  }

  failAll(error) {
    for (const pending of this.pending.values()) {
      pending.cleanup()
      pending.reject(error)
    }
    this.pending.clear()
  }

  async close() {
    try {
      this.child.stdin.end()
    } catch {
      // already closed
    }
    if (!this.exited) {
      const exited = new Promise((resolve) => this.child.once('exit', resolve))
      const timer = setTimeout(() => {
        try {
          this.child.kill('SIGKILL')
        } catch {
          // already gone
        }
      }, 3_000)
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5_000))])
      clearTimeout(timer)
    }
    this.failAll(new Error('tkless mcp closed'))
    this.reader.close()
  }
}

class TklessState {
  constructor(ctx, config) {
    this.ctx = ctx
    this.config = config ?? {}
    this.client = undefined
    this.binary = undefined
    this.state = 'idle'
    this.error = undefined
    this.toolCount = 0
    this.toolDisposers = []
    this.guardLift = undefined
    this.starting = undefined
    this.disposed = false
  }

  args() {
    const configured = this.config.args
    if (Array.isArray(configured) && configured.every((item) => typeof item === 'string')) {
      return configured
    }
    return DEFAULT_ARGS
  }

  prefix() {
    const configured = this.config.toolNamePrefix
    return typeof configured === 'string' && configured.length > 0 ? configured : DEFAULT_PREFIX
  }

  async start() {
    if (this.disposed) throw new Error('tkless-dsh has been disposed')
    if (this.starting) return this.starting
    this.starting = this.runStart().finally(() => {
      this.starting = undefined
    })
    return this.starting
  }

  async runStart() {
    this.state = 'starting'
    this.error = undefined
    try {
      const version = await packageVersion()
      const binary = await ensureBinary(this.config, version)
      if (this.disposed) throw new Error('tkless-dsh has been disposed')
      this.binary = binary
      log(`using tkless binary at ${binary}`)
      try {
        log(`tkless version: ${await probeBinary(binary)}`)
      } catch (error) {
        log(error?.message ?? error)
      }

      const env = {
        ...process.env,
        ...(this.config.env && typeof this.config.env === 'object' ? this.config.env : {}),
      }
      if (this.disposed) throw new Error('tkless-dsh has been disposed')
      const child = spawn(binary, this.args(), {
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
        env,
      })
      const client = new McpClient(child, (message) => this.onNotification(message), {
        protocolVersion: this.config.protocolVersion ?? '2026-07-28',
        clientInfo: { name: 'tkless-dsh', version },
      })
      this.client = client

      // MCP 2026-07-28 is stateless: there is no initialize handshake. Probe the
      // server and verify that it supports the version we send on every request.
      const discovered = await client.request('server/discover', {}, { timeoutMs: CONNECT_TIMEOUT_MS })
      const supported = Array.isArray(discovered?.supportedVersions)
        ? discovered.supportedVersions
        : []
      if (!supported.includes(client.protocolVersion)) {
        throw new Error(
          `tkless mcp does not support protocol ${client.protocolVersion}; supported: ${supported.join(', ') || 'unknown'}`,
        )
      }
      const serverInfo = discovered?._meta?.['io.modelcontextprotocol/serverInfo']
      log(`connected: ${serverInfo?.name ?? 'tkless'} ${serverInfo?.version ?? ''}`)

      const tools = await this.listTools()
      if (this.disposed) throw new Error('tkless-dsh has been disposed')
      this.registerTools(tools)
      this.installGuard()
      this.state = 'ready'
      log(`registered ${this.toolCount} tools`)
    } catch (error) {
      this.error = error
      this.state = 'error'
      await this.cleanupRuntime()
      log('startup failed:', errorSummary(error))
      throw error
    }
  }

  async listTools() {
    const tools = []
    let cursor
    do {
      const response = await this.client.request(
        'tools/list',
        cursor === undefined ? {} : { cursor },
        { timeoutMs: CONNECT_TIMEOUT_MS },
      )
      if (Array.isArray(response?.tools)) tools.push(...response.tools)
      cursor = typeof response?.nextCursor === 'string' ? response.nextCursor : undefined
    } while (cursor !== undefined)
    return tools
  }

  registerTools(tools) {
    this.disposeTools()
    for (const tool of tools) {
      if (!tool || typeof tool.name !== 'string' || tool.name.length === 0) continue
      const rawName = tool.name
      const name = `${this.prefix()}${rawName}`
      const definition = {
        name,
        description:
          typeof tool.description === 'string' && tool.description.length > 0
            ? tool.description
            : `TkLess ${rawName}`,
        parameters: normalizeInputSchema(tool.inputSchema),
        output: {
          schema: {
            type: 'object',
            properties: {
              text: { type: 'string' },
              structured: {
                oneOf: [
                  { type: 'object' },
                  { type: 'array' },
                  { type: 'string' },
                  { type: 'number' },
                  { type: 'boolean' },
                  { type: 'null' },
                ],
              },
            },
            required: ['text'],
            additionalProperties: false,
          },
          render: (_args, value) => [{ type: 'text', text: String(value?.text ?? '') }],
        },
        execute: async (args, exec) => {
          const { result, streamedItems } = await this.callTool(rawName, args, exec.signal)
          const text = renderResultText(result)
          const streamed = streamedItems.length > 0
            ? `\n\nStreamed items (${streamedItems.length}):\n${streamedItems
                .map((item) => JSON.stringify(item))
                .join('\n')}`
            : ''
          const value = { text: text + streamed }
          if (streamedItems.length > 0) {
            value.structured = { summary: result?.structuredContent ?? null, streamedItems }
          } else if (result?.structuredContent !== undefined) {
            value.structured = result.structuredContent
          }
          return value
        },
      }
      this.toolDisposers.push(this.ctx.tools.register(definition))
    }
    this.toolCount = this.toolDisposers.length
  }

  async callTool(rawName, args, signal) {
    if (!this.client || this.state !== 'ready') {
      throw new Error('tkless mcp is not ready; run tkless_dsh_status or tkless_dsh_retry')
    }
    const streamedItems = []
    const result = await this.client.request(
      'tools/call',
      { name: rawName, arguments: args ?? {} },
      {
        timeoutMs: CALL_TIMEOUT_MS,
        signal,
        onProgress: (params) => {
          const item = params?._meta?.['tkless/batch_item']
          if (item !== undefined) streamedItems.push(item)
        },
      },
    )
    // 2026-07-28 has no list_changed subscription here; activation changes the
    // tool list, so refresh before returning and before the next model step can
    // invoke a newly activated tool. `execute_tool` can also run activate_tools.
    const activates =
      rawName === 'activate_tools' ||
      (rawName === 'execute_tool' &&
        args !== null &&
        typeof args === 'object' &&
        args.tool_name === 'activate_tools')
    if (activates) {
      try {
        await this.refreshTools()
      } catch (error) {
        log('tool refresh after activation failed:', errorSummary(error))
      }
    }
    return { result, streamedItems }
  }

  installGuard() {
    if (this.guardLift || this.config.disableNativeSearch === false) return
    this.guardLift = this.ctx.tools.guard((exec) =>
      exec.name === 'web_search'
        ? 'web_search is disabled because TkLess handles web access. Use the mcp__tkless__ tools instead.'
        : undefined,
    )
    log('guard installed: web_search is denied')
  }

  onNotification(message) {
    // 2026-07-28 delivers list changes only through subscriptions/listen, which
    // TkLess does not expose. Progress and other request-scoped notifications are
    // not needed by the DSH bridge.
    void message
  }

  async refreshTools() {
    if (!this.client || this.state !== 'ready' || this.disposed) return
    const tools = await this.listTools()
    this.registerTools(tools)
    log(`refreshed tools: ${this.toolCount}`)
  }

  disposeTools() {
    for (const dispose of this.toolDisposers.splice(0)) {
      try {
        dispose()
      } catch (error) {
        log('tool disposer failed:', error?.message ?? error)
      }
    }
    this.toolCount = 0
  }

  async cleanupRuntime() {
    if (this.guardLift) {
      try {
        this.guardLift()
      } catch {
        // ignore
      }
      this.guardLift = undefined
    }
    this.disposeTools()
    const client = this.client
    this.client = undefined
    if (client) await client.close()
  }

  async restart() {
    if (this.starting) {
      try {
        await this.starting
      } catch {
        // ignore and retry
      }
    }
    await this.cleanupRuntime()
    this.state = 'idle'
    return this.start()
  }

  status(agent) {
    const visible = agent ? this.ctx.tools.get('web_search', agent) !== undefined : false
    return {
      state: this.state,
      protocolVersion:
        this.client?.protocolVersion ?? this.config.protocolVersion ?? '2026-07-28',
      tools: this.toolCount,
      binary: this.binary ?? binaryPathInPackage(),
      webSearchVisible: visible,
      error: this.error ? errorSummary(this.error) : null,
    }
  }

  async dispose() {
    this.disposed = true
    await this.cleanupRuntime()
    this.state = 'disposed'
  }
}

function registerControlTools(ctx, state) {
  ctx.effect(() =>
    ctx.tools.register({
      name: 'tkless_dsh_status',
      description:
        'Report the TkLess bridge state, the installed binary path, the number of exposed tools, and whether the built-in web_search is still visible to this agent.',
      parameters: { type: 'object', properties: {} },
      output: {
        schema: {
          type: 'object',
          properties: {
            state: { type: 'string' },
            protocolVersion: { type: 'string' },
            tools: { type: 'number' },
            binary: { type: 'string' },
            webSearchVisible: { type: 'boolean' },
            error: { oneOf: [{ type: 'string' }, { type: 'null' }] },
          },
          required: ['state', 'protocolVersion', 'tools', 'binary', 'error'],
          additionalProperties: false,
        },
        render: (_args, value) => [{ type: 'text', text: JSON.stringify(value, null, 2) }],
      },
      execute: (args, exec) => state.status(exec.agent),
    }),
  )

  ctx.effect(() =>
    ctx.tools.register({
      name: 'tkless_dsh_retry',
      description: 'Retry provisioning and starting the TkLess MCP bridge after a failed startup.',
      parameters: { type: 'object', properties: {} },
      output: {
        schema: {
          type: 'object',
          properties: {
            state: { type: 'string' },
            protocolVersion: { type: 'string' },
            tools: { type: 'number' },
            binary: { type: 'string' },
            webSearchVisible: { type: 'boolean' },
            error: { oneOf: [{ type: 'string' }, { type: 'null' }] },
          },
          required: ['state', 'protocolVersion', 'tools', 'binary', 'error'],
          additionalProperties: false,
        },
        render: (_args, value) => [{ type: 'text', text: JSON.stringify(value, null, 2) }],
      },
      execute: async (args, exec) => {
        await state.restart()
        return state.status(exec.agent)
      },
    }),
  )
}

export const inject = ['tools']

export async function apply(ctx, config = {}) {
  const state = new TklessState(ctx, config)
  ctx.effect(() => () => state.dispose())
  registerControlTools(ctx, state)
  try {
    await state.start()
  } catch (error) {
    log('tkless-dsh activated without a working bridge:', errorSummary(error))
  }
}
