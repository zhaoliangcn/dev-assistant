// ============================================================
// Dev-Assistant Web UI — Alpine.js 聊天组件
// ============================================================
// 依赖：Alpine.js 3.x + highlight.js（由 base.html 引入）
// 功能：WS 连接/重连、消息渲染（markdown + 语法高亮）、
//       可替换状态条、assistant 消息流式增量更新。

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
        // 状态条：thinking/status 事件显示在此（可替换，不进入消息列表）
        pendingStatus: null,

        init() {
            this.connectWS();
            this.initTheme();
        },

        // ── 主题 ──────────────────────────────────────────────────────

        // 读取本地偏好主题，缺省跟随系统 prefers-color-scheme
        theme: 'auto',
        systemDark: false,

        initTheme() {
            this.systemDark = window.matchMedia &&
                window.matchMedia('(prefers-color-scheme: dark)').matches;
            this.theme = localStorage.getItem('dev-assistant-theme') || 'auto';
            this.applyTheme();
        },

        applyTheme() {
            const dark = this.theme === 'dark' ||
                (this.theme === 'auto' && this.systemDark);
            document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
            // 同步 highlight.js 主题样式表（暗色用 github-dark，亮色用 github）
            const hlCss = document.getElementById('hljs-theme');
            if (hlCss) {
                hlCss.href = dark
                    ? 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css'
                    : 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css';
            }
        },

        toggleTheme() {
            if (this.theme === 'auto') {
                this.theme = this.systemDark ? 'light' : 'dark';
            } else {
                this.theme = this.theme === 'dark' ? 'light' : 'dark';
            }
            localStorage.setItem('dev-assistant-theme', this.theme);
            this.applyTheme();
        },

        themeLabel() {
            const dark = this.theme === 'dark' ||
                (this.theme === 'auto' && this.systemDark);
            return dark ? '☀️' : '🌙';
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
            };

            wsInstance.onclose = () => {
                self.connected = false;
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
                    this.addMessage('error', msg.content);
                    break;
                case 'done':
                    this.pendingStatus = null;
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
            if (!text) return;

            this.addMessage('user', text);
            this.input = '';

            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
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

        // 代码块高亮：优先用 highlight.js；未加载时回退为转义文本
        highlightCode(code, lang) {
            const escaped = this.escapeHtml(code);
            if (typeof window.hljs === 'undefined') {
                const cls = lang ? ' class="language-' + lang + '"' : '';
                return '<pre><code' + cls + '>' + escaped + '</code></pre>';
            }
            let html;
            try {
                const detected = lang && window.hljs.getLanguage(lang)
                    ? { language: lang }
                    : {};
                html = window.hljs.highlight(code, detected).value;
            } catch (e) {
                html = escaped;
            }
            const cls = lang ? ' class="language-' + lang + '"' : '';
            return '<pre><code' + cls + '>' + html + '</code></pre>';
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
