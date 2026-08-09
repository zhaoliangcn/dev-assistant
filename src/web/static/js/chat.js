// ============================================================
// Dev-Assistant Web UI — 聊天组件
// ============================================================
// 依赖：Alpine.js 3.x, theme.js, utils.js
// 功能：WS 连接/重连、消息渲染、会话管理、工具活动区

import { escapeHtml, renderMarkdown, copyTextToClipboard, lcsDiff } from './utils.js';

function chatApp() {
    let wsInstance = null;
    let reconnectTimer = null;
    let reconnectAttempts = 0;
    // 虚拟滚动窗口配置
    const VIRTUAL_WINDOW = 60; // 视口内保留消息数
    const OVERSCAN = 20; // 上下缓冲消息数

    return {
        connected: false,
        sessionId: null,
        messageId: 0,
        input: '',
        // C2: 对话区消息（user/assistant/system/error）
        chatMessages: [],
        // 虚拟滚动：完整列表（可能很大），仅渲染视口附近
        allChatMessages: [],
        virtualStart: 0,
        virtualEnd: VIRTUAL_WINDOW,
        // C2: 工具活动区消息（tool-call/tool-result）
        toolMessages: [],
        // 最近复制的消息 ID（用于"已复制"反馈，需在初始 state 声明以支持响应式）
        copiedMessageId: null,
        // 消息序号
        _msgSeq: 0,
        // D1: 侧栏 Tab（sessions / files）
        sidebarTab: 'sessions',
        // D2: 内嵌文件树状态
        treePath: '.',
        treeEntries: [],
        treeLoading: false,
        // W10: 是否正在生成
        busy: false,
        // S2: 会话历史列表
        sessions: [],
        loadingSessions: false,
        sessionsError: null,
        activeSessionId: null,
        // S4: 会话重命名编辑态
        renamingId: null,
        renameTitle: '',
        // 状态条
        pendingStatus: null,
        // Token 消耗累计
        tokenUsage: { prompt: 0, completion: 0, total: 0 },
        // ── 消息搜索（P2） ──
        searchQuery: '',
        searchResults: [],
        searchCurrentIndex: -1,
        isSearching: false,
        searchPanelOpen: false,
        // ── 连接状态与错误（P3） ──
        connectionStatus: 'disconnected', // disconnected | connecting | connected | error
        connectionError: null,
        reconnectCount: 0,

        init() {
            this.connectWS();
            this.loadSessions();
            // 加载模型列表和主题
            if (window.Alpine) {
                window.Alpine.store('models').load();
                window.Alpine.store('theme').init();
            }
            // 全局快捷键绑定
            this.bindGlobalShortcuts();
        },

        bindGlobalShortcuts() {
            // Ctrl+/ 或 Cmd+/：聚焦输入框
            document.addEventListener('keydown', (event) => {
                // Ctrl+/ 或 Cmd+/：聚焦输入框
                if ((event.ctrlKey || event.metaKey) && event.key === '/') {
                    event.preventDefault();
                    const input = document.querySelector('textarea[x-model="input"]');
                    if (input) input.focus();
                }
                // Escape：关闭搜索面板
                if (event.key === 'Escape' && this.searchPanelOpen) {
                    this.closeSearch();
                }
                // Ctrl+Shift+C：复制最后一条助手消息（仅非输入框聚焦时）
                if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key === 'C') {
                    const active = document.activeElement;
                    const isInput = active && (
                        active.tagName === 'INPUT' ||
                        active.tagName === 'TEXTAREA' ||
                        active.isContentEditable
                    );
                    if (!isInput) {
                        event.preventDefault();
                        this.copyLastAssistantMessage();
                    }
                }
                // Ctrl+N 或 Cmd+N：新对话
                if ((event.ctrlKey || event.metaKey) && event.key === 'n') {
                    const active = document.activeElement;
                    const isInput = active && (
                        active.tagName === 'INPUT' ||
                        active.tagName === 'TEXTAREA' ||
                        active.isContentEditable
                    );
                    if (!isInput) {
                        event.preventDefault();
                        this.newChat();
                    }
                }
            });
        },

        copyLastAssistantMessage() {
            const last = this.lastAssistantMessage();
            if (last) {
                this.copyMessage(last.content || '');
            }
        },

        // ── 拖拽上传（P3） ──

        initDragDrop() {
            const chatPanel = this.$refs.list?.parentElement;
            if (!chatPanel) return;

            ['dragenter', 'dragover', 'dragleave', 'drop'].forEach(eventName => {
                chatPanel.addEventListener(eventName, (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                });
            });

            chatPanel.addEventListener('dragenter', () => {
                this.dragOver = true;
            });

            chatPanel.addEventListener('dragleave', (e) => {
                if (!chatPanel.contains(e.relatedTarget)) {
                    this.dragOver = false;
                }
            });

            chatPanel.addEventListener('drop', (e) => {
                this.dragOver = false;
                const files = Array.from(e.dataTransfer?.files || []);
                if (files.length > 0) {
                    this.handleDroppedFiles(files);
                }
            });
        },

        async handleDroppedFiles(files) {
            for (const file of files) {
                try {
                    const content = await this.readFileAsText(file);
                    const message = `文件 ${file.name} 内容：\n\`\`\`\n${content}\n\`\`\``;
                    this.sendMessageWithContent(message);
                } catch (e) {
                    console.error('读取文件失败:', e);
                    this.addMessage('error', `无法读取文件 ${file.name}: ${e.message}`);
                }
            }
        },

        readFileAsText(file) {
            return new Promise((resolve, reject) => {
                const reader = new FileReader();
                reader.onload = () => resolve(reader.result);
                reader.onerror = () => reject(new Error('文件读取失败'));
                reader.readAsText(file);
            });
        },

        sendMessageWithContent(content) {
            if (this.activeSessionId) {
                this.newChat();
            }
            this.addMessage('user', content);
            this.busy = true;
            this.scrollToBottomLater();
            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: content,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            }
        },

        // ── 连接状态管理（P3） ──

        setConnectionStatus(status) {
            this.connectionStatus = status;
            if (status === 'connected') {
                this.connectionError = null;
                this.reconnectCount = 0;
            }
        },

        showConnectionError(message) {
            this.connectionError = message;
            this.connectionStatus = 'error';
            // 5 秒后自动清除错误
            setTimeout(() => {
                if (this.connectionError === message) {
                    this.connectionError = null;
                }
            }, 5000);
        },

        get connectionStatusLabel() {
            switch (this.connectionStatus) {
                case 'connected': return '在线';
                case 'connecting': return '连接中...';
                case 'disconnected': return '离线';
                case 'error': return '连接错误';
                default: return '未知';
            }
        },

        get connectionStatusIcon() {
            switch (this.connectionStatus) {
                case 'connected': return '🟢';
                case 'connecting': return '🟡';
                case 'disconnected': return '🔴';
                case 'error': return '⚠️';
                default: return '⚪';
            }
        },

        // ── 虚拟滚动 ──

        get visibleChatMessages() {
            const start = Math.max(0, this.virtualStart);
            const end = Math.min(this.allChatMessages.length, this.virtualEnd);
            return this.allChatMessages.slice(start, end);
        },

        get visibleStartIndex() {
            return Math.max(0, this.virtualStart);
        },

        get visibleEndIndex() {
            return Math.min(this.allChatMessages.length, this.virtualEnd);
        },

        get chatTotalCount() {
            return this.allChatMessages.length;
        },

        get showVirtualSpacerTop() {
            return this.visibleStartIndex > 0;
        },

        get showVirtualSpacerBottom() {
            return this.visibleEndIndex < this.allChatMessages.length;
        },

        get virtualSpacerTopHeight() {
            // 估算：每条消息平均 80px
            return this.visibleStartIndex * 80;
        },

        get virtualSpacerBottomHeight() {
            return Math.max(0, (this.allChatMessages.length - this.visibleEndIndex) * 80);
        },

        updateVirtualWindow() {
            if (this.allChatMessages.length <= VIRTUAL_WINDOW + OVERSCAN * 2) {
                // 消息少，全量渲染
                this.virtualStart = 0;
                this.virtualEnd = this.allChatMessages.length;
                return;
            }
            // 默认显示最后 N 条
            const end = this.allChatMessages.length;
            const start = Math.max(0, end - VIRTUAL_WINDOW - OVERSCAN);
            this.virtualStart = start;
            this.virtualEnd = end;
        },

        onChatScroll(event) {
            // 滚动时动态调整视口（可选增强）
            // 当前策略：默认显示最新消息，用户主动上滚时保留历史
            const el = event.target;
            const threshold = 200; // 距离底部 200px 内视为"查看最新"
            const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
            if (isNearBottom && this.allChatMessages.length > VIRTUAL_WINDOW + OVERSCAN * 2) {
                const end = this.allChatMessages.length;
                const start = Math.max(0, end - VIRTUAL_WINDOW - OVERSCAN);
                if (start !== this.virtualStart || end !== this.virtualEnd) {
                    this.virtualStart = start;
                    this.virtualEnd = end;
                }
            }
        },

        // ── 会话历史（S2） ──

        async loadSessions() {
            this.loadingSessions = true;
            this.sessionsError = null;
            try {
                const resp = await fetch('/api/sessions');
                if (!resp.ok) throw new Error('HTTP ' + resp.status);
                const data = await resp.json();
                this.sessions = Array.isArray(data) ? data : [];
            } catch (e) {
                console.error('加载会话列表失败:', e);
                this.sessions = [];
                this.sessionsError = e.message || '加载失败';
            } finally {
                this.loadingSessions = false;
            }
        },

        async selectSession(id) {
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id));
                if (!resp.ok) throw new Error('HTTP ' + resp.status);
                const data = await resp.json();
                this.activeSessionId = id;
                const all = this.eventsToMessages(data.events || []);
                this.allChatMessages = all.filter((m) => m.role !== 'tool-call' && m.role !== 'tool-result');
                // 与 allChatMessages 保持同一引用，避免扁平分支渲染旧会话的过期消息
                this.chatMessages = this.allChatMessages;
                this.toolMessages = all.filter((m) => m.role === 'tool-call' || m.role === 'tool-result');
                this.pendingStatus = null;
                this.updateVirtualWindow();
                this.scrollToBottomLater();
                this.scrollToolLater();
            } catch (e) {
                console.error('加载会话详情失败:', e);
            }
        },

        newChat() {
            this.chatMessages = [];
            this.allChatMessages = [];
            this.toolMessages = [];
            this.activeSessionId = null;
            this.pendingStatus = null;
            this.virtualStart = 0;
            this.virtualEnd = VIRTUAL_WINDOW;
        },

        deleteSession(id) {
            // 后端删除已由侧栏组件完成，这里仅同步本地会话列表状态
            this.sessions = this.sessions.filter((s) => s.id !== id);
            if (this.activeSessionId === id) {
                this.newChat();
            }
        },

        startRename(session) {
            this.renamingId = session.id;
            this.renameTitle = session.title || '';
        },

        cancelRename() {
            this.renamingId = null;
            this.renameTitle = '';
        },

        async submitRename(id) {
            const title = this.renameTitle.trim();
            if (!title) {
                this.cancelRename();
                return;
            }
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id) + '/rename', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ title: title }),
                });
                const data = await resp.json();
                if (data.success) {
                    const s = this.sessions.find((x) => x.id === id);
                    if (s) s.title = data.title;
                } else {
                    console.error('重命名失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('重命名会话失败:', e);
            } finally {
                this.cancelRename();
            }
        },

        // 侧栏组件重命名成功后经 window 事件通知，此处同步 chatApp 自身的会话列表
        renameSession(id, title) {
            const s = this.sessions.find((x) => x.id === id);
            if (s) s.title = title;
        },

        eventsToMessages(events) {
            return events
                .filter((ev) => ev && ev.type)
                .map((ev) => {
                    switch (ev.type) {
                        case 'user_message':
                            return { role: 'user', content: ev.content || '', time: ev.timestamp };
                        case 'assistant_message':
                            return { role: 'assistant', content: ev.content || '', time: ev.timestamp };
                        case 'system_message':
                            return { role: 'system', content: ev.content || '', time: ev.timestamp };
                        case 'tool_call_request':
                            return { role: 'tool-call', content: ev.name || '工具调用', time: ev.timestamp };
                        case 'tool_result':
                            return { role: 'tool-result', content: ev.content || '', time: ev.timestamp };
                        default:
                            return null;
                    }
                })
                .filter((m) => m !== null);
        },

        formatSessionTime(iso) {
            if (!iso) return '';
            const d = new Date(iso);
            if (isNaN(d.getTime())) return iso;
            const pad = (n) => String(n).padStart(2, '0');
            return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' +
                pad(d.getHours()) + ':' + pad(d.getMinutes());
        },

        // ── 侧栏文件树（D2） ──

        switchSidebarTab(tab) {
            this.sidebarTab = tab;
            if (tab === 'files' && this.treeEntries.length === 0) {
                this.loadFileDir('.');
            }
        },

        async loadFileDir(path) {
            this.treeLoading = true;
            this.treePath = path;
            try {
                const resp = await fetch('/api/files?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.treeEntries = data.entries || [];
            } catch (e) {
                console.error('加载文件树失败:', e);
                this.treeEntries = [];
            } finally {
                this.treeLoading = false;
            }
        },

        treeParent() {
            if (!this.treePath || this.treePath === '.') return '.';
            const idx = this.treePath.lastIndexOf('/');
            return idx <= 0 ? '.' : (this.treePath.slice(0, idx) || '.');
        },

        openSideFile(path) {
            window.location.href = '/files?path=' + encodeURIComponent(path);
        },

        formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
            return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
        },

        // ── 连接状态 ──

        setConnected(v) {
            if (window.Alpine) {
                window.Alpine.store('connection').connected = v;
            }
        },

        // ── WebSocket ──

        connectWS() {
            this.setConnectionStatus('connecting');
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
                self.setConnectionStatus('connected');
                self.reconnectCount = 0;
            };

            wsInstance.onclose = () => {
                self.connected = false;
                self.setConnected(false);
                self.setConnectionStatus('disconnected');
                if (reconnectAttempts < 10) {
                    const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000);
                    reconnectAttempts++;
                    self.reconnectCount = reconnectAttempts;
                    if (reconnectTimer) clearTimeout(reconnectTimer);
                    reconnectTimer = setTimeout(() => self.connectWS(), delay + Math.random() * 1000);
                } else {
                    self.showConnectionError('连接失败，请刷新页面重试');
                }
            };

            wsInstance.onerror = () => {
                self.showConnectionError('WebSocket 连接错误');
            };

            wsInstance.onmessage = (event) => {
                try {
                    const msg = JSON.parse(event.data);
                    self.handleServerEvent(msg);
                } catch (e) {
                    console.error('WebSocket 消息解析失败:', e);
                }
            };
        },

        // ── 消息管理 ──

        addMessage(role, content, extra) {
            const item = Object.assign({
                role: role,
                content: content,
                time: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
                _id: 'm' + (++this._msgSeq),
            }, extra || {});
            if (role === 'tool-call' || role === 'tool-result') {
                this.toolMessages = [...this.toolMessages, item];
                this.scrollToolLater();
            } else {
                this.allChatMessages = [...this.allChatMessages, item];
                this.chatMessages = this.allChatMessages;
                this.updateVirtualWindow();
                this.scrollToBottomLater();
            }
        },

        scrollToBottomLater() {
            this.$nextTick(() => {
                requestAnimationFrame(() => {
                    const el = this.$refs.list;
                    if (!el) return;
                    el.scrollTop = el.scrollHeight;
                });
            });
        },

        scrollToolLater() {
            this.$nextTick(() => {
                requestAnimationFrame(() => {
                    const el = this.$refs.toolList;
                    if (!el) return;
                    el.scrollTop = el.scrollHeight;
                });
            });
        },

        handleServerEvent(msg) {
            switch (msg.type) {
                case 'session_ready':
                    this.sessionId = msg.session_id;
                    break;
                case 'status':
                case 'thinking':
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
                case 'token_usage':
                    this.tokenUsage.prompt += msg.prompt_tokens || 0;
                    this.tokenUsage.completion += msg.completion_tokens || 0;
                    this.tokenUsage.total += msg.total_tokens || 0;
                    if (window.Alpine) {
                        window.Alpine.store('tokenUsage').add(
                            msg.prompt_tokens || 0,
                            msg.completion_tokens || 0,
                            msg.total_tokens || 0
                        );
                    }
                    break;
                case 'done':
                    this.pendingStatus = null;
                    this.busy = false;
                    this.finishStreaming();
                    break;
            }
        },

        finishStreaming() {
            const idx = this.lastAssistantIndex();
            if (idx >= 0 && this.allChatMessages[idx].streaming) {
                this.allChatMessages[idx] = {
                    role: 'assistant',
                    content: this.allChatMessages[idx].content,
                    streaming: false,
                    time: this.allChatMessages[idx].time,
                };
            }
        },

        appendAssistant(msg) {
            if (msg.streaming) {
                const idx = this.lastAssistantIndex();
                if (idx >= 0) {
                    const existing = this.allChatMessages[idx];
                    this.allChatMessages[idx] = {
                        role: 'assistant',
                        content: msg.content,
                        streaming: true,
                        time: existing.time,
                    };
                    this.chatMessages = this.allChatMessages;
                    this.updateVirtualWindow();
                    this.scrollToBottomLater();
                    return;
                }
            }
            this.addMessage('assistant', msg.content);
        },

        lastAssistantIndex() {
            for (let i = this.allChatMessages.length - 1; i >= 0; i--) {
                if (this.allChatMessages[i].role === 'assistant') return i;
            }
            return -1;
        },

        sendMessage() {
            const text = this.input.trim();
            if (!text || this.busy) return;

            // 处于历史会话查看态时，发送消息自动开启新对话
            if (this.activeSessionId) {
                this.newChat();
            }

            this.addMessage('user', text);
            this.input = '';
            this.busy = true;
            this.scrollToBottomLater();

            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            }
        },

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

        onInputKeydown(event) {
            if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
                event.preventDefault();
                this.sendMessage();
            }
        },

        // ── Markdown 渲染 ──

        formatContent(content) {
            if (!content) return '';
            return renderMarkdown(content);
        },

        // ── 复制消息 ──

        copyMessage(content) {
            copyTextToClipboard(content).then(() => {
                this.copiedMessageId = Date.now();
                setTimeout(() => { this.copiedMessageId = null; }, 1500);
            });
        },

        // ── 消息搜索 ──

        toggleSearch() {
            if (this.searchPanelOpen) {
                this.closeSearch();
            } else {
                this.openSearch();
            }
        },

        openSearch() {
            this.searchPanelOpen = true;
            this.searchQuery = '';
            this.searchResults = [];
            this.searchCurrentIndex = -1;
            // 自动聚焦搜索输入框
            this.$nextTick(() => {
                const input = document.querySelector('.search-input');
                if (input) input.focus();
            });
        },

        closeSearch() {
            this.searchPanelOpen = false;
            this.searchQuery = '';
            this.searchResults = [];
            this.searchCurrentIndex = -1;
        },

        performSearch() {
            const query = this.searchQuery.trim().toLowerCase();
            if (!query) {
                this.searchResults = [];
                this.searchCurrentIndex = -1;
                return;
            }
            this.isSearching = true;
            // 搜索所有聊天消息（包括历史和当前）
            const allMessages = [...this.allChatMessages];
            this.searchResults = allMessages
                .map((msg, index) => ({
                    index,
                    role: msg.role,
                    content: msg.content,
                    preview: this.getSearchPreview(msg.content, query),
                }))
                .filter((result) => result.preview !== null);
            this.searchCurrentIndex = this.searchResults.length > 0 ? 0 : -1;
            this.isSearching = false;
            // 滚动到第一个匹配项
            if (this.searchCurrentIndex >= 0) {
                this.scrollToMessage(this.searchResults[0].index);
            }
        },

        getSearchPreview(content, query) {
            const lowerContent = content.toLowerCase();
            const pos = lowerContent.indexOf(query);
            if (pos === -1) return null;
            // 提取匹配位置附近 50 个字符作为预览
            const start = Math.max(0, pos - 30);
            const end = Math.min(content.length, pos + query.length + 50);
            let preview = content.substring(start, end);
            if (start > 0) preview = '...' + preview;
            if (end < content.length) preview = preview + '...';
            // 高亮匹配文本
            const highlighted = preview.replace(
                new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'),
                '<mark>$&</mark>'
            );
            return highlighted;
        },

        scrollToMessage(index) {
            // 滚动到指定索引的消息
            // 先切换到正常视图（如果正在虚拟滚动）
            if (this.allChatMessages.length > 100) {
                // 临时调整虚拟窗口以显示目标消息
                const targetStart = Math.max(0, index - 10);
                const targetEnd = Math.min(this.allChatMessages.length, index + 10);
                this.virtualStart = targetStart;
                this.virtualEnd = targetEnd;
            }
            // 滚动到目标消息
            this.$nextTick(() => {
                const messageElements = document.querySelectorAll('#message-list .message');
                if (messageElements[index]) {
                    messageElements[index].scrollIntoView({ behavior: 'smooth', block: 'center' });
                }
            });
        },

        nextSearchResult() {
            if (this.searchResults.length === 0) return;
            this.searchCurrentIndex = (this.searchCurrentIndex + 1) % this.searchResults.length;
            const result = this.searchResults[this.searchCurrentIndex];
            this.scrollToMessage(result.index);
        },

        prevSearchResult() {
            if (this.searchResults.length === 0) return;
            this.searchCurrentIndex = (this.searchCurrentIndex - 1 + this.searchResults.length) % this.searchResults.length;
            const result = this.searchResults[this.searchCurrentIndex];
            this.scrollToMessage(result.index);
        },

        get searchResultCount() {
            return this.searchResults.length;
        },

        get searchHasResults() {
            return this.searchResults.length > 0;
        },

        // ── 会话导出（P5） ──

        async exportSession(format) {
            if (!this.activeSessionId) {
                alert('请先选择一个会话');
                return;
            }
            
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(this.activeSessionId) + '/export?format=' + format);
                if (!resp.ok) throw new Error('导出失败');
                
                const blob = await resp.blob();
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = 'session-' + this.activeSessionId + '.' + format;
                a.click();
                URL.revokeObjectURL(url);
            } catch (e) {
                console.error('导出会话失败:', e);
                alert('导出失败: ' + e.message);
            }
        },

        get canRetry() {
            // 仅在离线或连接错误时可重试
            return this.connectionStatus === 'disconnected' ||
                   this.connectionStatus === 'error';
        },

        retryLastAction() {
            if (!this.canRetry) return;
            // 重新连接 WebSocket
            if (wsInstance) {
                wsInstance.onclose = null;
                wsInstance.onerror = null;
                if (wsInstance.readyState === WebSocket.OPEN ||
                    wsInstance.readyState === WebSocket.CONNECTING) {
                    wsInstance.close();
                }
            }
            this.connectionError = null;
            this.reconnectAttempts = 0;
            this.connectWS();
            // 如果有待发送的消息，重新发送最后一条用户消息
            if (this.lastUserMessage && !this.busy) {
                this.sendMessageContent(this.lastUserMessage);
            }
        },

        // 记录最后一条用户消息（用于重试）
        lastUserMessage: '',

        sendMessage() {
            const text = this.input.trim();
            if (!text || this.busy) return;
            this.lastUserMessage = text;

            // 处于历史会话查看态时，发送消息自动开启新对话
            if (this.activeSessionId) {
                this.newChat();
            }

            this.addMessage('user', text);
            this.input = '';
            this.busy = true;
            this.scrollToBottomLater();

            this.sendMessageContent(text);
        },

        sendMessageContent(text) {
            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            } else {
                // 连接断开时显示错误
                this.showConnectionError('无法发送消息，连接已断开');
                this.busy = false;
            }
        },
    };
}

// ES module 顶层声明是模块作用域，需挂到 window 供 Alpine x-data 表达式解析
window.chatApp = chatApp;
