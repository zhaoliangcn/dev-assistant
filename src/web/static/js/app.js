// ============================================================
// Dev-Assistant Web UI — Alpine.js 聊天组件
// ============================================================

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

        init() {
            this.connectWS();
        },

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

        handleServerEvent(msg) {
            switch (msg.type) {
                case 'session_ready':
                    this.sessionId = msg.session_id;
                    break;
                case 'status':
                case 'thinking':
                    this.messages = [...this.messages, { role: msg.type, content: msg.content }];
                    break;
                case 'tool_call':
                    this.messages = [...this.messages,
                        { role: 'tool-call', content: '🔧 ' + (msg.tool_name || '操作') }
                    ];
                    break;
                case 'tool_result':
                    this.messages = [...this.messages,
                        { role: 'tool-result', content: msg.content }
                    ];
                    break;
                case 'assistant_message':
                    this.messages = [...this.messages, { role: 'assistant', content: msg.content }];
                    break;
                case 'error':
                    this.messages = [...this.messages, { role: 'error', content: '❌ ' + msg.content }];
                    break;
                case 'done':
                    break;
            }
        },

        sendMessage() {
            const text = this.input.trim();
            if (!text) return;

            this.messages = [...this.messages, { role: 'user', content: text }];
            this.input = '';

            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            }
        },

        formatContent(content) {
            if (!content) return '';
            let html = content
                .replace(/&/g, '&amp;')
                .replace(/</g, '&lt;')
                .replace(/>/g, '&gt;')
                .replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
                    const langClass = lang ? ' class="language-' + lang + '"' : '';
                    return '<pre><code' + langClass + '>' + this.escapeHtml(code.trim()) + '</code></pre>';
                })
                .replace(/`([^`]+)`/g, '<code>$1</code>')
                .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
                .replace(/\n/g, '<br>');
            return html;
        },

        escapeHtml(text) {
            return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
        }
    };
}
