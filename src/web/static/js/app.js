// ============================================================
// Dev-Assistant Web UI — Alpine.js 组件 + HTMX 扩展
// ============================================================

document.addEventListener('DOMContentLoaded', function() {
    // ── 代码高亮 ──
    // 在 HTMX 内容交换后重新应用 highlight.js
    document.body.addEventListener('htmx:afterSwap', function(evt) {
        evt.target.querySelectorAll('pre code').forEach(function(block) {
            hljs.highlightElement(block);
        });
    });

    // 初始加载时高亮
    document.querySelectorAll('pre code').forEach(function(block) {
        hljs.highlightElement(block);
    });
});

// ── Alpine.js 组件 ──

function chatApp() {
    return {
        connected: false,
        sessionId: null,

        init() {
            // 监听 WebSocket 连接状态
            const wsExt = htmx.findExtension('ws');
            if (wsExt) {
                this.connected = true;
            }

            // 监听自定义事件
            this.$el.addEventListener('ws:open', () => {
                this.connected = true;
            });
            this.$el.addEventListener('ws:close', () => {
                this.connected = false;
            });

            // 监视消息列表，自动滚动到底部
            this.$watch('$store.messages.length', () => {
                this.scrollToBottom();
            });
        },

        scrollToBottom() {
            const list = document.getElementById('message-list');
            if (list) {
                setTimeout(() => {
                    list.scrollTop = list.scrollHeight;
                }, 50);
            }
        },

        clearChat() {
            const list = document.getElementById('message-list');
            if (list) {
                // 保留欢迎消息
                const welcome = list.querySelector('.message.system');
                list.innerHTML = '';
                if (welcome) {
                    list.appendChild(welcome);
                }
            }
        },

        toggleSidebar() {
            // Phase 2: 切换文件浏览器侧栏
        }
    };
}

function toolbar() {
    return {
        connected: false,

        init() {
            // 检查 WebSocket 连接状态
            const wsExt = htmx.findExtension('ws');
            this.connected = wsExt ? true : false;

            document.body.addEventListener('htmx:wsOpen', () => {
                this.connected = true;
            });
            document.body.addEventListener('htmx:wsClose', () => {
                this.connected = false;
            });
        },

        clearChat() {
            const event = new CustomEvent('clear-chat');
            document.dispatchEvent(event);
        },

        toggleSidebar() {
            const sidebar = document.getElementById('sidebar');
            if (sidebar) {
                sidebar.style.display = sidebar.style.display === 'none' ? 'block' : 'none';
            }
        }
    };
}

// ── HTMX WebSocket 扩展配置 ──

htmx.defineExtension('ws', {
    init: function() {
        // 默认配置已满足需求
    },

    onEvent: function(name, evt) {
        if (name === 'htmx:wsOpen') {
            document.dispatchEvent(new CustomEvent('ws:open'));
        }
        if (name === 'htmx:wsClose') {
            document.dispatchEvent(new CustomEvent('ws:close'));
        }
    }
});