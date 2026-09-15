import express from 'express'
import { createProxyMiddleware } from 'http-proxy-middleware'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

const RUST_BACKEND = process.env.RUST_BACKEND || 'http://127.0.0.1:8080'
const PORT = parseInt(process.env.PORT || '3000', 10)
const DIST_DIR = join(__dirname, 'dist')

if (!existsSync(DIST_DIR)) {
  console.error('错误: dist 目录不存在，请先运行 npm run build')
  process.exit(1)
}

const app = express()

// 1. REST API 代理 -> Rust 后端
app.use(
  '/api',
  createProxyMiddleware({ target: RUST_BACKEND, changeOrigin: true })
)

// 2. WebSocket 代理 -> Rust 后端（http-proxy-middleware 自动处理 Upgrade 头）
//    注意：需要单独的 ws 路径代理
const wsProxy = createProxyMiddleware({
  target: RUST_BACKEND.replace(/^http/, 'ws'),
  changeOrigin: true,
  ws: true,
})
app.use('/ws', (req, res, next) => {
  wsProxy(req, res, next)
})

// 3. 静态文件：Vue 构建产物
app.use(express.static(DIST_DIR, { maxAge: '1h' }))

// 4. SPA fallback
app.get('*', (_req, res) => {
  res.sendFile(join(DIST_DIR, 'index.html'))
})

const server = app.listen(PORT, () => {
  console.log(`\n  Dev-Assistant Vue Web UI\n`)
  console.log(`  Local:   http://127.0.0.1:${PORT}`)
  console.log(`  Backend: ${RUST_BACKEND}\n`)
})

// WebSocket 代理需要监听 upgrade 事件
server.on('upgrade', (req, socket, head) => {
  if (req.url?.startsWith('/ws')) {
    wsProxy.upgrade(req, socket, head)
  }
})