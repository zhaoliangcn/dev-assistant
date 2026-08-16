// ============================================================
// Dev-Assistant Web UI — 文件浏览器组件
// ============================================================
// 依赖：Alpine.js 3.x, utils.js
// 功能：目录浏览、文件打开、编辑、预览、Diff

import { escapeHtmlText, renderMarkdown, lcsDiff } from './utils.js';

function fileExplorer() {
    return {
        entries: [],
        currentPath: '.',
        activePath: null,
        content: '',
        originalContent: '',
        modified: false,
        saved: false,
        saving: false,
        loading: false,
        viewMode: 'edit',
        previewHtml: '',
        diffLines: [],
        diffStats: null,
        // P3: 懒加载状态
        expandedDirs: new Set(['.']),
        lazyLoadingDirs: new Set(),
        // P3: 虚拟滚动
        treeVirtualStart: 0,
        treeVirtualWindow: 50,
        treeOverscan: 15,

        get visibleTreeEntries() {
            const start = Math.max(0, this.treeVirtualStart);
            const end = Math.min(this.entries.length, start + this.treeVirtualWindow + this.treeOverscan);
            return this.entries.slice(start, end);
        },

        get treeSpacerTop() {
            const start = Math.max(0, this.treeVirtualStart);
            return start * 32; // 每条约 32px
        },

        get treeSpacerBottom() {
            const end = Math.min(this.entries.length, this.treeVirtualStart + this.treeVirtualWindow + this.treeOverscan);
            return Math.max(0, (this.entries.length - end) * 32);
        },

        onTreeScroll(event) {
            const el = event.target;
            const threshold = 150;
            const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
            if (isNearBottom && this.entries.length > this.treeVirtualWindow + this.treeOverscan * 2) {
                const end = this.entries.length;
                const start = Math.max(0, end - this.treeVirtualWindow - this.treeOverscan);
                this.treeVirtualStart = start;
            }
        },

        async loadDir(path) {
            this.loading = true;
            this.currentPath = path;
            try {
                const resp = await fetch('/api/files?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.entries = data.entries || [];
                // 标记当前目录为已加载
                this.expandedDirs.add(path);
            } catch (e) {
                console.error('加载目录失败:', e);
                this.entries = [];
            } finally {
                this.loading = false;
                this.lazyLoadingDirs.delete(path);
            }
        },

        // P3: 懒加载子目录（仅当目录尚未加载时）
        async loadDirLazy(path) {
            if (this.expandedDirs.has(path) || this.lazyLoadingDirs.has(path)) {
                return;
            }
            this.lazyLoadingDirs.add(path);
            try {
                const resp = await fetch('/api/files?path=' + encodeURIComponent(path));
                const data = await resp.json();
                const entries = data.entries || [];
                // 在当前视图中插入或更新该目录的条目
                const existingIndex = this.entries.findIndex(e => e.path === path);
                if (existingIndex >= 0) {
                    // 移除占位符，插入实际条目
                    this.entries.splice(existingIndex, 1, ...entries);
                }
                this.expandedDirs.add(path);
            } catch (e) {
                console.error('懒加载目录失败:', e);
            } finally {
                this.lazyLoadingDirs.delete(path);
            }
        },

        // 判断目录是否已加载
        isDirLoaded(path) {
            return this.expandedDirs.has(path);
        },

        // 判断目录是否正在加载
        isDirLoading(path) {
            return this.lazyLoadingDirs.has(path);
        },

        parentPath() {
            if (!this.currentPath || this.currentPath === '.') return '.';
            const idx = this.currentPath.lastIndexOf('/');
            const parent = idx <= 0 ? '.' : this.currentPath.slice(0, idx);
            return parent || '.';
        },

        async newFile() {
            const name = prompt('输入新文件路径（相对项目目录，如 src/foo.rs）:', '');
            if (!name || !name.trim()) return;
            const path = name.trim().replace(/^\.\//, '');
            this.activePath = path;
            this.content = '';
            this.originalContent = '';
            this.modified = false;
            this.saved = false;
            this.viewMode = 'edit';
            this.previewHtml = '';
            this.diffLines = [];
            this.diffStats = null;
            const idx = path.lastIndexOf('/');
            if (idx > 0) {
                this.loadDir(path.slice(0, idx));
            }
        },

        async openFile(path) {
            this.activePath = path;
            this.modified = false;
            this.saved = false;
            this.viewMode = 'edit';
            try {
                const resp = await fetch('/api/files/content?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.content = data.content || '';
                this.originalContent = this.content;
                this.previewHtml = '';
                this.diffLines = [];
                this.diffStats = null;
            } catch (e) {
                console.error('加载文件失败:', e);
                this.content = '';
                this.originalContent = '';
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
                    this.originalContent = this.content;
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

        statusText() {
            if (this.saved) return '✅ 已保存';
            if (this.modified) return '📝 已修改';
            if (this.isNewFile()) return '🆕 新建文件';
            return '';
        },

        isNewFile() {
            return this.originalContent === '' && this.modified === false &&
                this.activePath !== null;
        },

        switchView(mode) {
            this.viewMode = mode;
            if (mode === 'preview') {
                this.previewHtml = this.renderPreview();
            } else if (mode === 'diff') {
                this.computeDiff();
            }
        },

        renderPreview() {
            const path = this.activePath || '';
            const isMarkdown = /\.(md|markdown|mdown)$/i.test(path);
            // D3：复用 utils.renderMarkdown（含任务列表/引用/代码高亮），删除本地重复实现
            if (isMarkdown) {
                return renderMarkdown(this.content);
            }
            const escaped = escapeHtmlText(this.content);
            if (typeof window.hljs !== 'undefined' && this.content.trim()) {
                try {
                    const langMatch = path.match(/\.([a-zA-Z0-9]+)$/);
                    const lang = langMatch ? langMatch[1] : '';
                    const detected = lang && window.hljs.getLanguage(lang)
                        ? { language: lang }
                        : {};
                    const highlighted = window.hljs.highlight(this.content, detected).value;
                    const cls = lang ? ' class="language-' + lang + '"' : '';
                    return '<pre class="preview-code"><code' + cls + '>' + highlighted + '</code></pre>';
                } catch (e) {
                    return '<pre class="preview-code"><code>' + escaped + '</code></pre>';
                }
            }
            return '<pre class="preview-code"><code>' + escaped + '</code></pre>';
        },

        computeDiff() {
            const oldLines = (this.originalContent || '').split('\n');
            const newLines = this.content.split('\n');
            const result = lcsDiff(oldLines, newLines);
            this.diffLines = result.lines;
            this.diffStats = result.stats;
        },

        formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
            return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
        },
    };
}

// ES module 顶层声明是模块作用域，需挂到 window 供 Alpine x-data 表达式解析
window.fileExplorer = fileExplorer;
