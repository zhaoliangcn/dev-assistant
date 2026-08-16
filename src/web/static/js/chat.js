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

    return {
        connected: false,
        sessionId: null,
        messageId: 0,
        input: '',
        // C2: 对话区消息（user/assistant/system/error）
        chatMessages: [],
        // C5: 完整消息列表（chatMessages 与之同源；浏览器级 content-visibility 负责廉价虚拟化）
        allChatMessages: [],
        // C2: 工具活动区消息（tool-call/tool-result）
        toolMessages: [],
        // 最近复制的消息 ID（用于"已复制"反馈，需在初始 state 声明以支持响应式）
        copiedMessageId: null,
        // 消息序号
        _msgSeq: 0,
        // C1: 流式增量渲染节流缓冲（非响应式用途，仅内部）
        _streamBuf: '',
        _streamFlushTimer: null,
        // D1: 侧栏 Tab（sessions / files）
        sidebarTab: 'sessions',
        // D2: 内嵌文件树状态
        treePath: '.',
        treeEntries: [],
        treeLoading: false,
        // W10: 是否正在生成
        busy: false,
        activeSessionId: null,
        // C4: 当前会话的事件总数 / 本次返回数（供"加载更多"判断）
        sessionTotal: 0,
        sessionReturned: 0,
        sessionLoading: false,
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
            // C6: 会话列表由 sidebarWidget.init 通过 $store.sessions.load() 统一拉取，
            // chatApp 不再重复请求（消除首页双重 /api/sessions 调用）
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
            // 用 lastAssistantIndex() 取最后一条助手消息（曾误用不存在的 lastAssistantMessage()）
            const idx = this.lastAssistantIndex();
            if (idx >= 0) {
                const msg = this.allChatMessages[idx];
                this.copyMessage(msg._id, msg.content || '');
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
            this.lastUserMessage = content;
            this.addMessage('user', content);
            this.busy = true;
            this.scrollToBottomLater();
            this.sendMessageContent(content);
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

        // ── 会话历史（S2） ──
        // C6+D1：列表加载/重命名/删除统一由 $store.sessions + sidebarWidget 负责，
        // chatApp 仅保留"选中会话→加载详情"与"删除当前会话→新建"两项响应。

        async selectSession(id) {
            this.sessionLoading = true;
            try {
                // C4: 首次仅拉最近 500 条，降低大对话的初始加载/渲染成本
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id) + '?limit=500');
                if (!resp.ok) throw new Error('HTTP ' + resp.status);
                const data = await resp.json();
                this.activeSessionId = id;
                this._applySessionData(data);
            } catch (e) {
                console.error('加载会话详情失败:', e);
            } finally {
                this.sessionLoading = false;
            }
        },

        // C4: 超过 limit 时，用户可点击"加载更多"拉取全部事件
        async loadFullSession() {
            if (!this.activeSessionId) return;
            this.sessionLoading = true;
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(this.activeSessionId));
                if (!resp.ok) throw new Error('HTTP ' + resp.status);
                const data = await resp.json();
                this._applySessionData(data);
            } catch (e) {
                console.error('加载完整会话失败:', e);
            } finally {
                this.sessionLoading = false;
            }
        },

        // 把后端 SessionDetail（含 total/returned/events）映射为前端消息视图
        _applySessionData(data) {
            const all = this.eventsToMessages(data.events || []);
            this.allChatMessages = all.filter((m) => m.role !== 'tool-call' && m.role !== 'tool-result');
            // 与 allChatMessages 保持同一引用，避免扁平分支渲染旧会话的过期消息
            this.chatMessages = this.allChatMessages;
            this.toolMessages = all.filter((m) => m.role === 'tool-call' || m.role === 'tool-result');
            this.sessionTotal = data.total || this.allChatMessages.length + this.toolMessages.length;
            this.sessionReturned = data.returned || data.events.length;
            this.pendingStatus = null;
            this.scrollToBottomLater();
            this.scrollToolLater();
        },

        newChat() {
            this.chatMessages = [];
            this.allChatMessages = [];
            this.toolMessages = [];
            this.activeSessionId = null;
            this.pendingStatus = null;
            this.sessionTotal = 0;
            this.sessionReturned = 0;
        },

        deleteSession(id) {
            // 列表维护已由 sidebarWidget 委托 $store.sessions.remove 完成；
            // chatApp 只需在删除的是当前会话时回到新建态
            if (this.activeSessionId === id) {
                this.newChat();
            }
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
                // 刷新重试时暂存的消息（retryLastAction 在建连前暂存）
                if (self._pendingRetryMessage) {
                    const msg = self._pendingRetryMessage;
                    self._pendingRetryMessage = null;
                    self.sendMessageContent(msg);
                }
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
                case 'assistant_stream_delta':
                    // C1: 增量流式——追加到当前 streaming 助手消息，节流渲染
                    this.pendingStatus = null;
                    this.applyStreamDelta(msg.delta || '', msg.is_final);
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
            // C1: done 到达时冲刷残留缓冲（防止终帧 delta 丢失），并清节流定时器
            if (this._streamFlushTimer) {
                clearTimeout(this._streamFlushTimer);
                this._streamFlushTimer = null;
            }
            const idx = this.lastStreamingAssistantIndex();
            if (idx < 0) {
                // 没有正在 streaming 的消息：兼容旧路径（非流式最终 assistant_message）
                const i = this.lastAssistantIndex();
                if (i >= 0 && this.allChatMessages[i].streaming) {
                    const cur = this.allChatMessages[i];
                    this.allChatMessages[i] = Object.assign({}, cur, { streaming: false });
                }
                return;
            }
            // 把残余缓冲并入并关闭 streaming 标志
            const buf = this._streamBuf || '';
            this._streamBuf = '';
            const cur = this.allChatMessages[idx];
            this.allChatMessages[idx] = Object.assign({}, cur, {
                content: (cur.content || '') + buf,
                streaming: false,
            });
            this.chatMessages = this.allChatMessages;
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

        // C1: 当前正在 streaming 的助手消息下标（无则 -1）
        lastStreamingAssistantIndex() {
            for (let i = this.allChatMessages.length - 1; i >= 0; i--) {
                const m = this.allChatMessages[i];
                if (m.role === 'assistant' && m.streaming) return i;
            }
            return -1;
        },

        // C1: 应用流式增量——追加到当前 streaming 助手消息，节流渲染。
        // 后端每 token 只下发 delta（非全量），前端缓冲后每 50ms 合并写入
        // content 一次，避免每 token 全量重解析 Markdown（O(n²)）。
        applyStreamDelta(delta, isFinal) {
            let idx = this.lastStreamingAssistantIndex();
            if (idx < 0) {
                // 首个增量：新建一条 streaming 助手消息（空内容 + 光标由 CSS 提供）
                this.addMessage('assistant', '', { streaming: true });
                idx = this.allChatMessages.length - 1;
            }
            this._streamBuf = (this._streamBuf || '') + (delta || '');
            if (isFinal) {
                this._flushStreamBuffer(idx, true);
            } else if (!this._streamFlushTimer) {
                this._streamFlushTimer = setTimeout(() => {
                    this._streamFlushTimer = null;
                    this._flushStreamBuffer(idx, false);
                }, 50);
            }
        },

        // 把缓冲区写入目标消息 content；isFinal 时关闭 streaming 标志。
        _flushStreamBuffer(idx, isFinal) {
            this._streamFlushTimer = null;
            const buf = this._streamBuf || '';
            this._streamBuf = '';
            if (idx >= this.allChatMessages.length) return;
            const cur = this.allChatMessages[idx];
            if (!cur) return;
            // 整体替换对象触发 Alpine 响应式重渲（content 变更 → x-html 重解析）
            this.allChatMessages[idx] = Object.assign({}, cur, {
                content: (cur.content || '') + buf,
                streaming: !isFinal,
            });
            this.chatMessages = this.allChatMessages;
            if (isFinal) {
                // 终帧：内容已齐，无需再滚动
                return;
            }
            this.scrollToBottomLater();
        },

        // 注意：sendMessage 只定义一次（历史误删重复定义曾导致聚焦等逻辑丢失）。

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
            // Enter / Ctrl+Enter / Cmd+Enter 发送；Shift+Enter 换行；IME 组合中不触发
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

        // 复制指定消息：传入消息 _id 与内容，copiedMessageId 存该 _id，
        // 按钮据此判断 copiedMessageId === msg._id 显示"已复制"（修共享状态 bug）。
        copyMessage(id, content) {
            copyTextToClipboard(content).then(() => {
                this.copiedMessageId = id;
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
            // 滚动到指定索引的消息（C5 后无虚拟窗口，DOM 始终全量存在）
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
            // 若有最后一条用户消息，暂存待重发——connectWS() 异步建连，
            // 必须等 onopen 后才能发送，否则会立即判为"连接已断开"。
            this._pendingRetryMessage = this.lastUserMessage || null;
            this.connectWS();
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

            // 发送后保持输入框聚焦，便于连续对话
            this.$nextTick(() => {
                const input = this.$refs.chatInput || document.querySelector('textarea[x-model="input"]');
                if (input) input.focus();
            });

            this.sendMessageContent(text);
        },

        // 低层发送：仅负责把消息写进 WebSocket，连接断开时提示错误。
        // 由 sendMessage / 拖拽上传 / 重试复用，保证发送语义一致。
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
