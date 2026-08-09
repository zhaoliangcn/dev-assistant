// ============================================================
// Dev-Assistant Web UI — 共享侧栏组件
// ============================================================
// 功能：会话列表 + 文件树，两个页面复用
// 通信：通过 Alpine event 向上传递选中/重命名/删除

import { escapeHtml } from './utils.js';

function sidebarWidget() {
    return {
        // Tab 状态
        activeTab: 'sessions',
        // 会话列表
        sessions: [],
        loadingSessions: false,
        sessionsError: null,
        activeSessionId: null,
        renamingId: null,
        renameTitle: '',
        // 文件树
        treePath: '.',
        treeEntries: [],
        treeLoading: false,

        init() {
            this.loadSessions();
        },

        // ── 会话管理 ──

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
            this.activeSessionId = id;
            // 向上传递选中事件
            this.$dispatch('session-selected', { id });
        },

        newChat() {
            this.activeSessionId = null;
            this.$dispatch('new-chat');
        },

        async deleteSession(id) {
            if (!confirm('确定删除该会话？')) return;
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id), {
                    method: 'DELETE',
                });
                const data = await resp.json();
                if (data.deleted) {
                    this.sessions = this.sessions.filter((s) => s.id !== id);
                    if (this.activeSessionId === id) {
                        this.activeSessionId = null;
                        this.$dispatch('new-chat');
                    }
                    this.$dispatch('session-deleted', { id });
                } else {
                    console.error('删除失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('删除会话失败:', e);
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
                    this.$dispatch('session-renamed', { id, title: data.title });
                } else {
                    console.error('重命名失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('重命名会话失败:', e);
            } finally {
                this.cancelRename();
            }
        },

        formatSessionTime(iso) {
            if (!iso) return '';
            const d = new Date(iso);
            if (isNaN(d.getTime())) return iso;
            const pad = (n) => String(n).padStart(2, '0');
            return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' +
                pad(d.getHours()) + ':' + pad(d.getMinutes());
        },

        // ── 文件树 ──

        switchTab(tab) {
            this.activeTab = tab;
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

        openFile(path) {
            this.$dispatch('file-opened', { path });
        },

        formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
            return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
        },
    };
}

// ES module 顶层声明是模块作用域，需挂到 window 供 Alpine x-data 表达式解析
window.sidebarWidget = sidebarWidget;
