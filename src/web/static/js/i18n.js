// ============================================================
// Dev-Assistant Web UI — 国际化 i18n 支持
// ============================================================
// 支持语言：中文（zh）、英文（en）
// 用法：在 Alpine store 中通过 $store.i18n.send 获取翻译文本
//       切换语言时所有绑定自动更新（响应式）

const TRANSLATIONS = {
    zh: {
        // 通用
        app_name: 'Dev-Assistant',
        loading: '加载中...',
        error: '错误',
        success: '成功',
        cancel: '取消',
        confirm: '确认',
        delete: '删除',
        rename: '重命名',
        save: '保存',
        close: '关闭',
        search: '搜索',
        // 导航
        online: '在线',
        offline: '离线',
        chat: '对话',
        files: '文件',
        menu: '菜单',
        model: '模型',
        theme: '切换主题',
        // 聊天
        chat_placeholder: '输入消息... (Enter 发送，Shift+Enter 换行)',
        send: '发送',
        stop: '停止',
        new_chat: '新对话',
        thinking: '正在思考...',
        tool_call: '工具调用',
        tool_result: '工具结果',
        assistant: '助手',
        user: '你',
        system: '系统',
        error_message: '错误',
        // 会话
        session_history: '会话历史',
        no_sessions: '暂无历史会话',
        session_count: '条消息',
        delete_confirm: '确定删除该会话？',
        rename_title: '标题',
        rename_placeholder: '输入新标题',
        // 文件
        file_browser: '文件浏览器',
        file_tree: '文件树',
        new_file: '新建文件',
        file_edit: '编辑',
        file_preview: '预览',
        file_diff: 'Diff',
        file_saved: '已保存',
        file_modified: '已修改',
        file_new: '新建文件',
        file_loading: '加载中...',
        file_empty: '（空目录）',
        // 搜索
        search_placeholder: '搜索消息...',
        search_no_results: '未找到匹配结果',
        search_prev: '上一个',
        search_next: '下一个',
        // 连接
        connecting: '连接中...',
        connection_error: '连接错误',
        reconnect_failed: '连接失败，请刷新页面重试',
        theme_auto: '主题：自动（当前{0}）',
        theme_dark: '主题：深色',
        theme_light: '主题：浅色',
        // 性能
        performance: '性能监控',
        fps: 'FPS',
        memory: '内存',
        // 快捷键
        shortcut_focus: 'Ctrl+/ 聚焦输入',
        shortcut_new: 'Ctrl+N 新对话',
        shortcut_copy: 'Ctrl+Shift+C 复制最后一条回复',
    },
    en: {
        app_name: 'Dev-Assistant',
        loading: 'Loading...',
        error: 'Error',
        success: 'Success',
        cancel: 'Cancel',
        confirm: 'Confirm',
        delete: 'Delete',
        rename: 'Rename',
        save: 'Save',
        close: 'Close',
        search: 'Search',
        online: 'Online',
        offline: 'Offline',
        chat: 'Chat',
        files: 'Files',
        menu: 'Menu',
        model: 'Model',
        theme: 'Toggle Theme',
        chat_placeholder: 'Type a message... (Enter to send, Shift+Enter for newline)',
        send: 'Send',
        stop: 'Stop',
        new_chat: 'New Chat',
        thinking: 'Thinking...',
        tool_call: 'Tool Call',
        tool_result: 'Tool Result',
        assistant: 'Assistant',
        user: 'You',
        system: 'System',
        error_message: 'Error',
        session_history: 'Session History',
        no_sessions: 'No sessions yet',
        session_count: 'msgs',
        delete_confirm: 'Delete this session?',
        rename_title: 'Title',
        rename_placeholder: 'Enter new title',
        file_browser: 'File Browser',
        file_tree: 'File Tree',
        new_file: 'New File',
        file_edit: 'Edit',
        file_preview: 'Preview',
        file_diff: 'Diff',
        file_saved: 'Saved',
        file_modified: 'Modified',
        file_new: 'New File',
        file_loading: 'Loading...',
        file_empty: '(empty)',
        search_placeholder: 'Search messages...',
        search_no_results: 'No results found',
        search_prev: 'Previous',
        search_next: 'Next',
        connecting: 'Connecting...',
        connection_error: 'Connection Error',
        reconnect_failed: 'Connection failed. Please refresh.',
        theme_auto: 'Theme: Auto (current {0})',
        theme_dark: 'Theme: Dark',
        theme_light: 'Theme: Light',
        performance: 'Performance',
        fps: 'FPS',
        memory: 'Memory',
        shortcut_focus: 'Ctrl+/ Focus Input',
        shortcut_new: 'Ctrl+N New Chat',
        shortcut_copy: 'Ctrl+Shift+C Copy Last Reply',
    }
};

// Alpine store 初始化
document.addEventListener('alpine:init', () => {
    if (!window.Alpine) return;

    // 构建初始翻译属性（中文）
    const initialProps = {};
    for (const key of Object.keys(TRANSLATIONS.zh)) {
        initialProps[key] = TRANSLATIONS.zh[key];
    }

    window.Alpine.store('i18n', {
        currentLang: 'zh',
        ...initialProps,

        setLanguage(lang) {
            if (!TRANSLATIONS[lang]) return;
            this.currentLang = lang;
            // 更新所有响应式翻译属性 → Alpine 自动重新渲染所有绑定
            for (const key of Object.keys(TRANSLATIONS[lang])) {
                this[key] = TRANSLATIONS[lang][key];
            }
        },

        toggle() {
            this.setLanguage(this.currentLang === 'zh' ? 'en' : 'zh');
        },

        get availableLanguages() {
            return [
                { code: 'zh', name: '中文' },
                { code: 'en', name: 'English' },
            ];
        },
    });
});