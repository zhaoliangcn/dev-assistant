<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(defineProps<{
  oldText: string
  newText: string
  /** 显示行号 */
  showLineNumbers?: boolean
}>(), {
  showLineNumbers: true,
})

interface DiffLine {
  type: 'ctx' | 'add' | 'del'
  marker: ' ' | '+' | '-'
  text: string
  oldNum?: number
  newNum?: number
}

interface DiffResult {
  lines: DiffLine[]
  stats: { add: number; del: number; ctx: number }
}

/** LCS 算法计算行级 diff */
function lcsDiff(oldStr: string, newStr: string): DiffResult {
  const oldLines = oldStr ? oldStr.split('\n') : []
  const newLines = newStr ? newStr.split('\n') : []
  const n = oldLines.length
  const m = newLines.length

  const dp: number[][] = []
  for (let i = 0; i <= n; i++) {
    dp.push(new Array(m + 1).fill(0))
  }
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      if (oldLines[i] === newLines[j]) {
        dp[i][j] = dp[i + 1][j + 1] + 1
      } else {
        dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1])
      }
    }
  }

  const lines: DiffLine[] = []
  let i = 0
  let j = 0
  let add = 0
  let del = 0
  let ctx = 0
  let oldNum = 1
  let newNum = 1

  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      lines.push({ type: 'ctx', marker: ' ', text: oldLines[i], oldNum, newNum })
      ctx++
      i++
      j++
      oldNum++
      newNum++
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      lines.push({ type: 'del', marker: '-', text: oldLines[i], oldNum })
      del++
      i++
      oldNum++
    } else {
      lines.push({ type: 'add', marker: '+', text: newLines[j], newNum })
      add++
      j++
      newNum++
    }
  }
  while (i < n) {
    lines.push({ type: 'del', marker: '-', text: oldLines[i], oldNum })
    del++
    i++
    oldNum++
  }
  while (j < m) {
    lines.push({ type: 'add', marker: '+', text: newLines[j], newNum })
    add++
    j++
    newNum++
  }

  return { lines, stats: { add, del, ctx } }
}

const diff = computed(() => lcsDiff(props.oldText, props.newText))
const hasChanges = computed(() => diff.value.stats.add > 0 || diff.value.stats.del > 0)
</script>

<template>
  <div class="diff-view">
    <div class="diff-stats">
      <span class="stat-add">+{{ diff.stats.add }}</span>
      <span class="stat-del">-{{ diff.stats.del }}</span>
      <span v-if="!hasChanges" class="stat-unchanged">无变化</span>
    </div>
    <div class="diff-body">
      <div v-for="(line, idx) in diff.lines" :key="idx" :class="['diff-line', `diff-line--${line.type}`]">
        <span v-if="showLineNumbers" class="diff-line-num">
          {{ line.type === 'add' ? line.newNum ?? '' : line.type === 'del' ? line.oldNum ?? '' : line.newNum ?? '' }}
        </span>
        <span class="diff-marker">{{ line.marker }}</span>
        <span class="diff-text">{{ line.text }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.diff-view {
  border-radius: 8px;
  overflow: hidden;
  border: 1px solid var(--color-border);
  background: var(--color-code-bg);
  margin: 8px 0;
}

.diff-stats {
  display: flex;
  gap: 8px;
  padding: 4px 12px;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
  font-size: 11px;
  font-weight: 600;
}

.stat-add { color: var(--color-success); }
.stat-del { color: var(--color-danger); }
.stat-unchanged { color: var(--color-text-muted); font-weight: 400; }

.diff-body {
  overflow-x: auto;
  font-family: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace;
  font-size: 13px;
}

.diff-line {
  display: flex;
  align-items: baseline;
  padding: 0;
  white-space: pre;
}

.diff-line--add {
  background: color-mix(in srgb, var(--color-success) 12%, transparent);
}

.diff-line--del {
  background: color-mix(in srgb, var(--color-danger) 12%, transparent);
}

.diff-line--ctx {
  opacity: 0.8;
}

.diff-line-num {
  flex-shrink: 0;
  width: 40px;
  padding: 0 8px;
  text-align: right;
  color: var(--color-text-muted);
  font-size: 11px;
  user-select: none;
}

.diff-marker {
  flex-shrink: 0;
  width: 20px;
  padding: 0 4px;
  text-align: center;
  font-weight: 700;
  user-select: none;
}

.diff-line--add .diff-marker { color: var(--color-success); }
.diff-line--del .diff-marker { color: var(--color-danger); }

.diff-text {
  flex: 1;
  padding-right: 12px;
}
</style>
