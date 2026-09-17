<script setup lang="ts">
import { ref, onMounted } from 'vue'
import {
  listSkills,
  installSkills,
  previewSkills,
  removeSkill,
  type SkillEntry,
} from '@/api/skills'

const skills = ref<SkillEntry[]>([])
const loading = ref(false)
const message = ref('')
const messageOk = ref(false)

// 安装表单
const source = ref('')
const scope = ref('project')
const installing = ref(false)

// 预览结果
const previewList = ref<SkillEntry[]>([])
const selected = ref<Set<string>>(new Set())
const previewing = ref(false)

async function refresh() {
  loading.value = true
  try {
    const res = await listSkills('all')
    skills.value = res.skills
  } catch (e) {
    showMessage(`加载失败: ${(e as Error).message}`, false)
  } finally {
    loading.value = false
  }
}

function showMessage(text: string, ok: boolean) {
  message.value = text
  messageOk.value = ok
  if (ok) setTimeout(() => { if (messageOk.value) message.value = '' }, 8000)
}

async function doPreview() {
  if (!source.value.trim()) { showMessage('请输入 source（如 owner/repo）', false); return }
  previewing.value = true
  previewList.value = []
  selected.value = new Set()
  try {
    const res = await previewSkills(source.value.trim())
    previewList.value = res.skills
    selected.value = new Set(res.skills.map((s) => s.name))
    if (!res.skills.length) showMessage('该来源中未发现技能（SKILL.md）', false)
  } catch (e) {
    showMessage(`预览失败: ${(e as Error).message}`, false)
  } finally {
    previewing.value = false
  }
}

function toggle(name: string) {
  const s = new Set(selected.value)
  if (s.has(name)) s.delete(name)
  else s.add(name)
  selected.value = s
}

async function doInstall() {
  if (!source.value.trim()) { showMessage('请输入 source（如 owner/repo）', false); return }
  installing.value = true
  try {
    const names = previewList.value.length ? [...selected.value] : undefined
    const res = await installSkills(source.value.trim(), names, scope.value)
    const namesStr = res.installed.map((s) => s.name).join(', ')
    showMessage(`✅ 已安装 ${res.installed.length} 个技能（${scope.value}）: ${namesStr}`, true)
    previewList.value = []
    selected.value = new Set()
    await refresh()
  } catch (e) {
    showMessage(`安装失败: ${(e as Error).message}`, false)
  } finally {
    installing.value = false
  }
}

async function doRemove(skill: SkillEntry) {
  if (!confirm(`确定移除技能「${skill.name}」（${skill.scope}）？`)) return
  try {
    await removeSkill(skill.name, skill.scope)
    showMessage(`已移除 ${skill.name}`, true)
    await refresh()
  } catch (e) {
    showMessage(`移除失败: ${(e as Error).message}`, false)
  }
}

onMounted(refresh)
</script>

<template>
  <div class="skills-view">
    <h1>技能管理</h1>

    <section class="install-card">
      <h2>安装技能</h2>
      <div class="form-row">
        <input
          v-model="source"
          placeholder="owner/repo 或完整 Git URL 或本地目录"
          @keyup.enter="doPreview"
        />
        <button :disabled="previewing" @click="doPreview">
          {{ previewing ? '预览中…' : '预览' }}
        </button>
      </div>
      <div class="form-row">
        <label class="scope-label">
          <input type="radio" value="project" v-model="scope" /> 项目级
        </label>
        <label class="scope-label">
          <input type="radio" value="global" v-model="scope" /> 全局级
        </label>
        <button class="primary" :disabled="installing" @click="doInstall">
          {{ installing ? '安装中…' : '安装' }}
        </button>
      </div>

      <div v-if="previewList.length" class="preview-list">
        <h3>可安装技能（{{ previewList.length }}）</h3>
        <label v-for="s in previewList" :key="s.name" class="preview-item">
          <input
            type="checkbox"
            :checked="selected.has(s.name)"
            @change="toggle(s.name)"
          />
          <strong>{{ s.name }}</strong>
          <span class="desc">{{ s.description }}</span>
        </label>
      </div>
    </section>

    <section class="installed-card">
      <h2>已安装（{{ skills.length }}）</h2>
      <p v-if="loading">加载中…</p>
      <p v-else-if="!skills.length" class="empty">暂无已安装技能</p>
      <div v-else class="skill-grid">
        <div v-for="s in skills" :key="s.scope + '/' + s.name" class="skill-card">
          <div class="skill-head">
            <strong>{{ s.name }}</strong>
            <span class="badge" :class="s.scope">{{ s.scope === 'global' ? '全局' : '项目' }}</span>
            <span v-if="s.version" class="badge version">v{{ s.version }}</span>
          </div>
          <p class="desc">{{ s.description }}</p>
          <p v-if="s.when_to_use" class="when">触发: {{ s.when_to_use }}</p>
          <p v-if="s.source" class="source" :title="s.source">{{ s.source }}</p>
          <button class="danger" @click="doRemove(s)">移除</button>
        </div>
      </div>
    </section>

    <div v-if="message" class="message" :class="{ ok: messageOk }">{{ message }}</div>
  </div>
</template>

<style scoped>
.skills-view { padding: 24px; max-width: 960px; margin: 0 auto; }
h1 { font-size: 1.4rem; margin-bottom: 16px; }
h2 { font-size: 1.05rem; margin: 0 0 12px; }
.install-card, .installed-card {
  border: 1px solid var(--border, #ddd); border-radius: 8px;
  padding: 16px; margin-bottom: 20px;
}
.form-row { display: flex; gap: 8px; margin-bottom: 10px; align-items: center; }
.form-row input[type='text'], .form-row input:not([type]) {
  flex: 1; padding: 8px 10px; border: 1px solid var(--border, #ccc); border-radius: 6px;
}
.scope-label { display: flex; align-items: center; gap: 4px; white-space: nowrap; }
button {
  padding: 8px 14px; border: 1px solid var(--border, #ccc); border-radius: 6px;
  background: var(--bg, #fff); cursor: pointer;
}
button:disabled { opacity: 0.5; cursor: not-allowed; }
button.primary { background: #3b82f6; color: #fff; border-color: #3b82f6; }
button.danger { color: #dc2626; border-color: #fca5a5; font-size: 0.85rem; padding: 4px 10px; }
.preview-list { margin-top: 12px; border-top: 1px dashed var(--border, #ddd); padding-top: 10px; }
.preview-item { display: flex; align-items: center; gap: 8px; padding: 4px 0; cursor: pointer; }
.preview-item .desc { color: var(--text-secondary, #666); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.skill-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 12px; }
.skill-card {
  border: 1px solid var(--border, #e5e5e5); border-radius: 8px; padding: 12px;
  display: flex; flex-direction: column; gap: 6px;
}
.skill-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.badge {
  font-size: 0.72rem; padding: 1px 8px; border-radius: 10px;
  background: #e5e7eb; color: #374151;
}
.badge.global { background: #dbeafe; color: #1d4ed8; }
.badge.project { background: #dcfce7; color: #15803d; }
.desc { margin: 0; font-size: 0.88rem; color: var(--text-secondary, #555); }
.when { margin: 0; font-size: 0.78rem; color: #92400e; }
.source {
  margin: 0; font-size: 0.75rem; color: #999;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.empty { color: #999; }
.message {
  position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%);
  background: #fee2e2; color: #991b1b; padding: 10px 20px; border-radius: 8px;
  max-width: 80vw;
}
.message.ok { background: #dcfce7; color: #166534; }
</style>
