// ============================================================
// Dev-Assistant Web UI — Alpine.js 聊天组件
// ============================================================
// 依赖：Alpine.js 3.x + highlight.js（由 base.html 引入）
// 功能：WS 连接/重连、消息渲染（markdown + 语法高亮）、
//       可替换状态条、assistant 消息流式增量更新。

// W7: 全局状态 store — header 在线指示器与主题按钮位于 chatApp 的
// x-data 作用域之外，因此连接/主题状态通过 Alpine store 共享。
document.addEventListener('alpine:init', () => {
    if (!window.Alpine) return;

    window.Alpine.store('connection', { connected: false });

    window.Alpine.store('theme', {
        theme: localStorage.getItem('dev-assistant-theme') || 'auto',
        systemDark: false,

        init() {
            this.systemDark = window.matchMedia &&
                window.matchMedia('(prefers-color-scheme: dark)').matches;
            this.apply();
        },

        get dark() {
            return this.theme === 'dark' ||
                (this.theme === 'auto' && this.systemDark);
        },

        // 当前主题图标（按钮显示）
        label() {
            return this.dark ? '☀️' : '🌙';
        },

        // 主题模式提示（按钮 title）：自动 / 深色 / 浅色
        modeLabel() {
            if (this.theme === 'auto') return '主题：自动（当前' + (this.dark ? '深色' : '浅色') + '）';
            return this.theme === 'dark' ? '主题：深色' : '主题：浅色';
        },

        // 应用 data-theme + 同步 highlight.js 主题样式表
        apply() {
            document.documentElement.setAttribute('data-theme', this.dark ? 'dark' : 'light');
            const hlCss = document.getElementById('hljs-theme');
            if (hlCss) {
                hlCss.href = this.dark
                    ? 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css'
                    : 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css';
            }
        },

        // 循环切换：auto → dark → light → auto
        toggle() {
            if (this.theme === 'auto') {
                this.theme = this.systemDark ? 'light' : 'dark';
            } else if (this.theme === 'dark') {
                this.theme = 'light';
            } else {
                this.theme = 'auto';
            }
            localStorage.setItem('dev-assistant-theme', this.theme);
            this.apply();
        },
    });
});

function chatApp() {
    let wsInstance = null;
    let reconnectTimer = null;
    let reconnectAttempts = 0;

    return {
        connected: false,
        sessionId: null,
        messageId: 0,
        input: '',
        messages: [],
        // W10: 是否正在生成（停止按钮显示）
        busy: false,
        // 状态条：thinking/status 事件显示在此（可替换，不进入消息列表）
        pendingStatus: null,

        init() {
            this.connectWS();
            // 主题由全局 store 管理（header 按钮与聊天区共享）
            if (window.Alpine) {
                window.Alpine.store('theme').init();
            }
        },

        // ── 连接状态 ──────────────────────────────────────────────────

        // W7: 同步连接状态到全局 store（header 指示器）
        setConnected(v) {
            if (window.Alpine) {
                window.Alpine.store('connection').connected = v;
            }
        },

        // ── WebSocket ─────────────────────────────────────────────────

        connectWS() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const url = protocol + '//' + window.location.host + '/ws/chat';

            if (wsInstance) {
                wsInstance.onclose = null;
                wsInstance.onerror = null;
                if (wsInstance.readyState === WebSocket.OPEN ||
                    wsInstance.readyState === WebSocket.CONNECTING) {
                    wsInstance.close();
                }
            }

            const self = this;
            wsInstance = new WebSocket(url);

            wsInstance.onopen = () => {
                self.connected = true;
                reconnectAttempts = 0;
                self.setConnected(true);
            };

            wsInstance.onclose = () => {
                self.connected = false;
                self.setConnected(false);
                if (reconnectAttempts < 10) {
                    const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000);
                    reconnectAttempts++;
                    if (reconnectTimer) clearTimeout(reconnectTimer);
                    reconnectTimer = setTimeout(() => self.connectWS(), delay + Math.random() * 1000);
                }
            };

            wsInstance.onerror = () => {};

            wsInstance.onmessage = (event) => {
                try {
                    const msg = JSON.parse(event.data);
                    self.handleServerEvent(msg);
                } catch (e) {
                    console.error('WebSocket 消息解析失败:', e);
                }
            };
        },

        // ── 服务端事件处理 ────────────────────────────────────────────

        // 追加消息并附带时间戳（消息头显示用）
        addMessage(role, content, extra) {
            this.messages = [...this.messages, Object.assign({
                role: role,
                content: content,
                time: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
            }, extra || {})];
            this.$nextTick(() => this.scrollToBottom(false));
        },

        // W5: 消息更新后自动滚动到底部。
        // 用户上翻阅读历史（距底部超过阈值）时不打扰；force=true 强制滚动
        // （用于用户主动发送消息的场景）。
        scrollToBottom(force) {
            const el = this.$refs.list;
            if (!el) return;
            const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
            if (!force && !nearBottom) return;
            el.scrollTop = el.scrollHeight;
        },

        handleServerEvent(msg) {
            switch (msg.type) {
                case 'session_ready':
                    this.sessionId = msg.session_id;
                    break;
                case 'status':
                case 'thinking':
                    // W2: 状态/思考显示为可替换状态条，不进入消息列表
                    this.pendingStatus = { role: msg.type, content: msg.content };
                    break;
                case 'tool_call':
                    this.pendingStatus = null;
                    this.addMessage('tool-call', msg.tool_name || '操作');
                    break;
                case 'tool_result':
                    this.pendingStatus = null;
                    this.addMessage('tool-result', msg.content);
                    break;
                case 'assistant_message':
                    this.pendingStatus = null;
                    this.appendAssistant(msg);
                    break;
                case 'error':
                    this.pendingStatus = null;
                    this.busy = false;
                    this.addMessage('error', msg.content);
                    break;
                case 'done':
                    this.pendingStatus = null;
                    this.busy = false;
                    this.finishStreaming();
                    break;
            }
        },

        // 流式结束后移除最后一条 assistant 消息的 streaming 边框
        finishStreaming() {
            const idx = this.lastAssistantIndex();
            if (idx >= 0 && this.messages[idx].streaming) {
                this.messages[idx] = {
                    role: 'assistant',
                    content: this.messages[idx].content,
                    streaming: false,
                    time: this.messages[idx].time,
                };
            }
        },

        // W3: assistant 消息流式增量更新。
        // 服务端 `streaming: true` 事件携带的是**累积的完整内容**（见
        // MessageOutput::streaming_assistant 约定），因此前端只需替换
        // 最后一条 assistant 消息的 content，无需字符串拼接。
        appendAssistant(msg) {
            if (msg.streaming) {
                const idx = this.lastAssistantIndex();
                if (idx >= 0) {
                    this.messages[idx] = {
                        role: 'assistant',
                        content: msg.content,
                        streaming: true,
                        time: this.messages[idx].time,
                    };
                    // 流式跟随：用户若停留在底部则实时滚动
                    this.$nextTick(() => this.scrollToBottom(false));
                    return;
                }
                // 找不到进行中的 assistant 消息则按完整消息追加
            }
            this.addMessage('assistant', msg.content);
        },

        lastAssistantIndex() {
            for (let i = this.messages.length - 1; i >= 0; i--) {
                if (this.messages[i].role === 'assistant') return i;
            }
            return -1;
        },

        sendMessage() {
            const text = this.input.trim();
            if (!text || this.busy) return;

            this.addMessage('user', text);
            this.input = '';
            this.busy = true;
            // 用户主动发送：强制滚动到底部
            this.$nextTick(() => this.scrollToBottom(true));

            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            }
        },

        // W10: 停止生成 — 发送 Cancel 事件，服务端中断 agent 任务并回 done
        stopGeneration() {
            if (!wsInstance || wsInstance.readyState !== WebSocket.OPEN) {
                this.busy = false;
                return;
            }
            this.pendingStatus = null;
            this.busy = false;
            wsInstance.send(JSON.stringify({
                type: 'cancel',
                message_id: 'msg_' + Date.now() + '_' + (this.messageId++)
            }));
        },

        // W8: textarea 按键处理 — Enter 发送，Shift+Enter 换行。
        // isComposing 排除中文输入法选字时的 Enter 确认。
        onInputKeydown(event) {
            if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
                event.preventDefault();
                this.sendMessage();
            }
        },

        // ── Markdown 渲染（W1） ────────────────────────────────────────

        // 安全转义：所有用户/模型内容先转义再注入 x-html
        escapeHtml(text) {
            return text
                .replace(/&/g, '&amp;')
                .replace(/</g, '&lt;')
                .replace(/>/g, '&gt;')
                .replace(/"/g, '&quot;');
        },

        // 行内格式：行内码 / 粗体 / 链接（在已转义文本上做结构化替换）
        inlineFormat(line) {
            let out = this.escapeHtml(line);
            // 行内代码 `x` — 最优先，避免被后续规则误伤
            out = out.replace(/`([^`]+)`/g, (_, code) => '<code>' + code + '</code>');
            // 粗体 **x**
            out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
            // 链接 [text](url) — 仅允许 http/https 协议，防 javascript: 注入
            out = out.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
                (_, text, url) => '<a href="' + url + '" target="_blank" rel="noopener">' + text + '</a>');
            return out;
        },

        // 代码块高亮：优先用 highlight.js；未加载时回退为转义文本。
        // 外层包复制按钮（onclick 委托给全局 copyCode）。
        highlightCode(code, lang) {
            const escaped = this.escapeHtml(code);
            let html;
            if (typeof window.hljs === 'undefined') {
                html = escaped;
            } else {
                try {
                    const detected = lang && window.hljs.getLanguage(lang)
                        ? { language: lang }
                        : {};
                    html = window.hljs.highlight(code, detected).value;
                } catch (e) {
                    html = escaped;
                }
            }
            const cls = lang ? ' class="language-' + lang + '"' : '';
            const langLabel = lang ? lang : 'code';
            return '<div class="code-block">' +
                '<div class="code-block-header">' +
                '<span class="code-block-lang">' + this.escapeHtml(langLabel) + '</span>' +
                '<button type="button" class="copy-btn" onclick="copyCode(this)">📋 复制</button>' +
                '</div>' +
                '<pre><code' + cls + '>' + html + '</code></pre>' +
                '</div>';
        },

        // W6: 复制消息全文（消息头复制按钮）
        copyMessage(content) {
            const self = this;
            copyTextToClipboard(content).then(() => {
                self.copiedMessageId = Date.now();
                setTimeout(() => { self.copiedMessageId = null; }, 1500);
            });
        },

        formatContent(content) {
            if (!content) return '';
            const lines = content.split('\n');
            const out = [];
            let i = 0;

            while (i < lines.length) {
                const line = lines[i];

                // 围栏代码块 ```lang ... ```
                const fence = line.match(/^```(\w*)/);
                if (fence) {
                    const lang = fence[1];
                    const buf = [];
                    i++;
                    while (i < lines.length && !lines[i].startsWith('```')) {
                        buf.push(lines[i]);
                        i++;
                    }
                    i++; // 跳过结束围栏
                    out.push(this.highlightCode(buf.join('\n'), lang));
                    continue;
                }

                // 标题 # ~ ######
                const heading = line.match(/^(#{1,6})\s+(.*)$/);
                if (heading) {
                    const level = Math.min(heading[1].length + 1, 6);
                    out.push('<h' + level + '>' + this.inlineFormat(heading[2]) + '</h' + level + '>');
                    i++;
                    continue;
                }

                // 引用 > text
                if (line.startsWith('>')) {
                    const buf = [];
                    while (i < lines.length && lines[i].startsWith('>')) {
                        buf.push(this.inlineFormat(lines[i].replace(/^>\s?/, '')));
                        i++;
                    }
                    out.push('<blockquote>' + buf.join('<br>') + '</blockquote>');
                    continue;
                }

                // 无序列表 - / * / + item
                const ul = line.match(/^[-*+]\s+(.*)$/);
                if (ul) {
                    const items = [];
                    while (i < lines.length) {
                        const m = lines[i].match(/^[-*+]\s+(.*)$/);
                        if (!m) break;
                        items.push('<li>' + this.inlineFormat(m[1]) + '</li>');
                        i++;
                    }
                    out.push('<ul>' + items.join('') + '</ul>');
                    continue;
                }

                // 有序列表 1. item
                const ol = line.match(/^\d+\.\s+(.*)$/);
                if (ol) {
                    const items = [];
                    while (i < lines.length) {
                        const m = lines[i].match(/^\d+\.\s+(.*)$/);
                        if (!m) break;
                        items.push('<li>' + this.inlineFormat(m[1]) + '</li>');
                        i++;
                    }
                    out.push('<ol>' + items.join('') + '</ol>');
                    continue;
                }

                // 表格 | a | b |（含分隔行 | --- | --- |）
                if (line.startsWith('|') && lines[i + 1] && /^\|[\s:|-]+\|/.test(lines[i + 1])) {
                    const rows = [];
                    while (i < lines.length && lines[i].startsWith('|')) {
                        const cells = lines[i]
                            .trim()
                            .replace(/^\||\|$/g, '')
                            .split('|')
                            .map((c) => c.trim());
                        rows.push(cells);
                        i++;
                    }
                    if (rows.length > 1) {
                        // 第二行是分隔行（| --- | --- |），跳过
                        const header = rows[0];
                        const body = rows.slice(2);
                        let html = '<table><thead><tr>';
                        header.forEach((c) => { html += '<th>' + this.inlineFormat(c) + '</th>'; });
                        html += '</tr></thead><tbody>';
                        body.forEach((row) => {
                            html += '<tr>';
                            header.forEach((_, ci) => {
                                html += '<td>' + this.inlineFormat(row[ci] || '') + '</td>';
                            });
                            html += '</tr>';
                        });
                        html += '</tbody></table>';
                        out.push(html);
                    }
                    continue;
                }

                // 空行
                if (line.trim() === '') {
                    if (out.length && !out[out.length - 1].endsWith('<br>')) {
                        out.push('<br>');
                    }
                    i++;
                    continue;
                }

                // 普通段落
                out.push('<p>' + this.inlineFormat(line) + '</p>');
                i++;
            }

            return out.join('\n');
        }
    };
}

// ── 文件浏览器组件（W11） ──────────────────────────────────────────
// 对接现有 /api/files* 接口，替代依赖未加载 htmx 的半成品模板。

function fileExplorer() {
    return {
        entries: [],
        currentPath: '.',
        activePath: null,
        content: '',
        modified: false,
        saved: false,
        saving: false,
        loading: false,

        async loadDir(path) {
            this.loading = true;
            this.currentPath = path;
            try {
                const resp = await fetch('/api/files?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.entries = data.entries || [];
            } catch (e) {
                console.error('加载目录失败:', e);
                this.entries = [];
            } finally {
                this.loading = false;
            }
        },

        async openFile(path) {
            this.activePath = path;
            this.modified = false;
            this.saved = false;
            try {
                const resp = await fetch('/api/files/content?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.content = data.content || '';
            } catch (e) {
                console.error('加载文件失败:', e);
                this.content = '';
            }
        },

        onEdit() {
            this.modified = true;
            this.saved = false;
        },

        async saveFile() {
            if (!this.activePath || this.saving) return;
            this.saving = true;
            try {
                const resp = await fetch('/api/files/save', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ path: this.activePath, content: this.content }),
                });
                const data = await resp.json();
                if (data.success) {
                    this.modified = false;
                    this.saved = true;
                    setTimeout(() => { this.saved = false; }, 1500);
                } else {
                    console.error('保存失败:', data.error);
                }
            } catch (e) {
                console.error('保存失败:', e);
            } finally {
                this.saving = false;
            }
        },

        formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
            return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
        },
    };
}

// ── 全局复制工具（W6） ─────────────────────────────────────────────

// 异步复制文本到剪贴板；回退到 document.execCommand（旧浏览器/非 https）
function copyTextToClipboard(text) {
    if (navigator.clipboard && window.isSecureContext) {
        return navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
    }
    return Promise.resolve(fallbackCopy(text));
}

function fallbackCopy(text) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    try {
        document.execCommand('copy');
    } catch (e) {
        console.error('复制失败:', e);
    }
    document.body.removeChild(ta);
}

// 代码块复制按钮：从按钮向上找到 .code-block 容器内的 <code> 文本
function copyCode(btn) {
    const block = btn.closest('.code-block');
    if (!block) return;
    const code = block.querySelector('pre code');
    const text = code ? code.innerText : '';
    copyTextToClipboard(text).then(() => {
        btn.textContent = '✅ 已复制';
        setTimeout(() => { btn.textContent = '📋 复制'; }, 1500);
    });
}

