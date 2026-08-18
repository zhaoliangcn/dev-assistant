// ============================================================
// Dev-Assistant Web UI — 共享侧栏组件
// ============================================================
// 功能：会话列表 + 文件树，两个页面复用
// 数据：会话列表统一委托 `$store.sessions`（单一数据源），
//       本组件只持有 UI 态（Tab、高亮、重命名编辑）。
// 数据：会话列表统一委托 `$store.sessions`（单一数据源），
//       本组件只持有 UI 态（Tab、高亮、重命名编辑）。
// 通信：通过 Alpine event 向上传递选中/重命名/删除

import { escapeHtml } from './utils.js';

function sidebarWidget() {
    return {
        // Tab 状态
        activeTab: 'sessions',
        // UI 态（不持有会话列表本体，避免与 $store.sessions 重复）
        // UI 态（不持有会话列表本体，避免与 $store.sessions 重复）
        activeSessionId: null,
        renamingId: null,
        renameTitle: '',
        // 文件树
        treePath: '.',
        treeEntries: [],
        treeLoading: false,

        // C6+D1：会话列表委托全局 store（仅保留模板仍引用的 sessions getter）
        get sessions() {
            return this.$store.sessions.list;
        },

        init() {
            // 仅此处触发初始加载（chatApp 不再重复调用）
            this.$store.sessions.load();
            // 仅此处触发初始加载（chatApp 不再重复调用）
            this.$store.sessions.load();
        },

        // ── 会话管理 ──

        async loadSessions() {
            await this.$store.sessions.load();
        },

        selectSession(id) {
            await this.$store.sessions.load();
        },

        selectSession(id) {
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
            const ok = await this.$store.sessions.remove(id);
            if (ok) {
                if (this.activeSessionId === id) {
                    this.activeSessionId = null;
                    this.$dispatch('new-chat');
                }
                this.$dispatch('session-deleted', { id });
            const ok = await this.$store.sessions.remove(id);
            if (ok) {
                if (this.activeSessionId === id) {
                    this.activeSessionId = null;
                    this.$dispatch('new-chat');
                }
                this.$dispatch('session-deleted', { id });
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
            const newTitle = await this.$store.sessions.rename(id, title);
            if (newTitle) {
                this.$dispatch('session-renamed', { id, title: newTitle });
            }
            this.cancelRename();
            const newTitle = await this.$store.sessions.rename(id, title);
            if (newTitle) {
                this.$dispatch('session-renamed', { id, title: newTitle });
            }
            this.cancelRename();
        },

        formatSessionTime(iso) {
            return this.$store.sessions.formatTime(iso);
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
            return this.$store.sessions.formatTime(iso);
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
