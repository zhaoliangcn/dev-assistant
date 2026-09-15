import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

type Locale = 'zh' | 'en'

const translations: Record<Locale, Record<string, string>> = {
  zh: {
    online: '已连接',
    offline: '离线',
    model: '模型',
    model_settings: '模型设置',
    language: '语言',
    theme_toggle: '切换主题',
    light_mode: '亮色模式',
    dark_mode: '暗色模式',
    config_file: '配置文件',
    model_settings_hint: '配置模型 URL、提供商与 API 密钥，保存后立即生效。',
    active: '当前',
    api_key_set: '已设置 API Key',
    api_key_missing: '未设置 API Key',
    no_models: '暂无模型配置',
    add_model: '新增模型',
    edit_model: '编辑模型',
    model_name: '名称',
    model_name_placeholder: '唯一标识，如 primary',
    provider: '提供商',
    api_url: 'API URL',
    api_key: 'API Key',
    api_key_keep: '留空保持不变',
    clear_api_key: '清空 API Key',
    temperature: 'Temperature',
    max_output_tokens: '最大输出 Tokens',
    reasoning_effort: '推理力度',
    reasoning_effort_hint: '越高思考越深但更慢；留空沿用默认',
    form_required: '请填写名称、提供商、API URL 和模型名',
    save: '保存',
    delete: '删除',
    cancel: '取消',
    close: '关闭',
    saving: '保存中...',
    delete_confirm: '确定删除该模型？',
    // 项目目录
    project_dir: '项目目录',
    welcome_select_project: '选择或输入项目目录后开始对话',
    current_project: '当前项目',
    project_dir_placeholder: '粘贴或输入项目目录路径...',
    switch_project: '选择项目目录',
    working_dir_hint: '服务器工作目录',
    project_invalid: '项目目录无效或超出服务器工作目录范围',
    switching_project: '正在切换到项目目录…',
  },
  en: {
    online: 'Online',
    offline: 'Offline',
    model: 'Model',
    model_settings: 'Model Settings',
    language: 'Language',
    theme_toggle: 'Toggle Theme',
    light_mode: 'Light Mode',
    dark_mode: 'Dark Mode',
    config_file: 'Config File',
    model_settings_hint: 'Configure model URL, provider and API key. Changes take effect immediately.',
    active: 'Active',
    api_key_set: 'API Key Set',
    api_key_missing: 'API Key Missing',
    no_models: 'No models configured',
    add_model: 'Add Model',
    edit_model: 'Edit Model',
    model_name: 'Name',
    model_name_placeholder: 'unique id, e.g. primary',
    provider: 'Provider',
    api_url: 'API URL',
    api_key: 'API Key',
    api_key_keep: 'leave empty to keep unchanged',
    clear_api_key: 'Clear API Key',
    temperature: 'Temperature',
    max_output_tokens: 'Max Output Tokens',
    reasoning_effort: 'Reasoning Effort',
    reasoning_effort_hint: 'higher = deeper thinking but slower; empty = server default',
    form_required: 'Name, provider, API URL and model are required',
    save: 'Save',
    delete: 'Delete',
    cancel: 'Cancel',
    close: 'Close',
    saving: 'Saving...',
    delete_confirm: 'Delete this model?',
    // Project directory
    project_dir: 'Project',
    welcome_select_project: 'Select a project directory to start chatting',
    current_project: 'Current project',
    project_dir_placeholder: 'Paste or type a project path...',
    switch_project: 'Switch Project',
    working_dir_hint: 'Server working directory',
    project_invalid: 'Project directory is invalid or outside server working directory',
    switching_project: 'Switching project directory…',
  },
}

export function useI18n() {
  const app = useAppStore()
  const locale = computed(() => app.locale)

  function t(key: string): string {
    return translations[locale.value]?.[key] ?? key
  }

  return { locale, t }
}
