// ============================================================
// Dev-Assistant Web UI — 共享工具函数
// ============================================================
// 功能：HTML 转义、Markdown 渲染、代码高亮、复制、Diff 计算

// ── HTML 转义 ──

/**
 * 安全转义 HTML 特殊字符，防止 XSS。
 * @param {string} text
 * @returns {string}
 */
export function escapeHtml(text) {
    return String(text)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

/**
 * 转义文本（别名，保持与旧代码兼容）。
 * @param {string} text
 * @returns {string}
 */
export function escapeHtmlText(text) {
    return escapeHtml(text);
}

// ── Markdown 行内格式 ──

/**
 * 对已转义文本应用行内格式：行内代码、粗体、链接、删除线。
 * @param {string} line - 已转义的文本
 * @returns {string}
 */
export function inlineFormat(line) {
    let out = escapeHtml(line);
    // 行内代码 `x`
    out = out.replace(/`([^`]+)`/g, (_, code) => '<code>' + code + '</code>');
    // 粗体 **x**（先于斜体处理，避免 ** 被拆成两个 *）
    out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    // 斜体 *x*（粗体已消耗成对 **，剩余单 * 即斜体）
    out = out.replace(/\*([^*\s][^*]*)\*/g, '<em>$1</em>');
    // 斜体 _x_（仅当两侧是词边界，避免 snake_case 被误伤）
    out = out.replace(/(^|[\s(>])_([^_\s][^_]*?)_(?=$|[\s).,;:!?\u3002\uff0c<b]|\b)/g, '$1<em>$2</em>');
    // 删除线 ~~x~~
    out = out.replace(/~~([^~]+)~~/g, '<del>$1</del>');
    // 链接 [text](url)
    out = out.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
        (_, text, url) => '<a href="' + url + '" target="_blank" rel="noopener">' + text + '</a>');
    return out;
}

// ── 代码高亮 ──

/**
 * 对代码块进行高亮处理，返回 HTML。
 * @param {string} code - 原始代码
 * @param {string} lang - 语言标识
 * @param {number} collapseThreshold - 超过此行数折叠（默认 20）
 * @returns {string}
 */
export function highlightCode(code, lang, collapseThreshold = 20) {
    const escaped = escapeHtml(code);
    let html;
    if (typeof window.hljs === 'undefined') {
        html = escaped;
    } else {
        try {
            const detected = lang && window.hljs.getLanguage(lang)
                ? { language: lang }
                : {};
            html = window.hljs.highlight(code, detected).value;
        } catch (e) {
            html = escaped;
        }
    }
    const cls = lang ? ' class="language-' + lang + '"' : '';
    const langLabel = lang ? lang : 'code';
    const lineCount = code.split('\n').length;
    const collapsible = lineCount > collapseThreshold;
    const actions = (collapsible
        ? '<button type="button" class="copy-btn" onclick="window._daToggleCodeBlock(this)">展开</button>'
        : '') +
        '<button type="button" class="copy-btn" onclick="window._daCopyCode(this)">📋 复制</button>' +
        // P5: 代码块运行按钮（仅 JavaScript）
        (lang === 'javascript' || lang === 'js'
            ? ' <button type="button" class="run-btn" onclick="window._daRunCode(this)">▶ 运行</button>'
            : '');
    return '<div class="code-block' + (collapsible ? ' code-block-collapsed' : '') + '">' +
        '<div class="code-block-header">' +
        '<span class="code-block-lang">' + escapeHtml(langLabel) + '</span>' +
        '<div class="code-block-actions">' + actions + '</div>' +
        '</div>' +
        '<pre><code' + cls + '>' + html + '</code></pre>' +
        '</div>';
}

// ── Markdown 列表解析（支持嵌套） ──

/**
 * 解析一段连续列表行为嵌套 HTML。
 * 以 2 个空格为一级缩进；缩进大于当前层的行递归处理为子列表。
 * @param {string[]} lines - 列表行（可含后续非列表行，遇到即停止）
 * @returns {{ html: string, consumed: number }}
 */
function renderListBlock(lines) {
    const itemRe = /^(\s*)([-*+]|\d+\.)\s+(.*)$/;
    // 预扫描：确定本块消费多少行
    let consumed = 0;
    for (let i = 0; i < lines.length; i++) {
        if (itemRe.test(lines[i]) || /^\s+\S/.test(lines[i])) {
            consumed = i + 1;
        } else {
            break;
        }
    }
    const block = lines.slice(0, consumed);

    // 逐项解析为 {indent, ordered, text}，缩进 >= 当前层 +2 的归入上一条的子项
    const parseItems = (arr) => arr.map((l) => {
        const m = l.match(itemRe);
        return m ? { indent: m[1].length, ordered: /^\d/.test(m[2]), text: m[3] } : null;
    }).filter(Boolean);

    const build = (items) => {
        if (items.length === 0) return '';
        const ordered = items[0].ordered;
        let html = ordered ? '<ol>' : '<ul>';
        let i = 0;
        while (i < items.length) {
            const it = items[i];
            const baseIndent = it.indent;
            // 收集比当前项更深缩进的连续子项
            const children = [];
            let j = i + 1;
            while (j < items.length && items[j].indent > baseIndent) {
                children.push({ ...items[j], indent: items[j].indent - baseIndent - 2 });
                j++;
            }
            const taskMatch = it.text.match(/^\[([ xX])\]\s*(.*)$/);
            let content;
            if (taskMatch) {
                const checked = taskMatch[1] === 'x' || taskMatch[1] === 'X';
                content = '<input type="checkbox" ' + (checked ? 'checked' : '') + ' disabled> ' +
                    inlineFormat(taskMatch[2]);
                html += '<li class="task-item" data-checked="' + checked + '">' + content;
            } else {
                html += '<li>' + inlineFormat(it.text);
            }
            if (children.length) html += build(children);
            html += '</li>';
            i = j;
        }
        html += ordered ? '</ol>' : '</ul>';
        return html;
    };

    return { html: build(parseItems(block)), consumed };
}

// ── 轻量流式渲染 ──

/**
 * 流式期间的廉价渲染：仅转义 + 换行，不解析 Markdown、不做代码高亮。
 * 终帧到达后用 renderMarkdown 全量渲染一次替换，避免每 50ms
 * 对整条消息重复 O(n) 解析与 hljs 高亮（长回复越写越卡）。
 * @param {string} content
 * @returns {string}
 */
export function renderPlain(content) {
    if (!content) return '';
    return escapeHtml(content).replace(/\n/g, '<br>');
}

// ── Markdown 渲染 ──

/**
 * 将 Markdown 文本渲染为 HTML（支持代码块、标题、引用、列表、表格）。
 * @param {string} content
 * @returns {string}
 */
export function renderMarkdown(content) {
    if (!content) return '';
    const lines = content.split('\n');
    const out = [];
    let i = 0;

    while (i < lines.length) {
        const line = lines[i];

        // 围栏代码块 ```lang ... ```
        const fence = line.match(/^```(\w*)/);
        if (fence) {
            const lang = fence[1];
            const buf = [];
            i++;
            while (i < lines.length && !lines[i].startsWith('```')) {
                buf.push(lines[i]);
                i++;
            }
            i++; // 跳过结束围栏
            out.push(highlightCode(buf.join('\n'), lang));
            continue;
        }

        // 标题 # ~ ######
        const heading = line.match(/^(#{1,6})\s+(.*)$/);
        if (heading) {
            const level = Math.min(heading[1].length + 1, 6);
            out.push('<h' + level + '>' + inlineFormat(heading[2]) + '</h' + level + '>');
            i++;
            continue;
        }

        // 引用 > text
        if (line.startsWith('>')) {
            const buf = [];
            while (i < lines.length && lines[i].startsWith('>')) {
                buf.push(inlineFormat(lines[i].replace(/^>\s?/, '')));
                i++;
            }
            out.push('<blockquote>' + buf.join('<br>') + '</blockquote>');
            continue;
        }

        // 列表（无序 -/*/+ 或有序 1.，支持缩进嵌套与任务列表）
        if (/^(\s*)([-*+]|\d+\.)\s+/.test(line)) {
            const { html, consumed } = renderListBlock(lines.slice(i));
            if (consumed > 0) {
                out.push(html);
                i += consumed;
                continue;
            }
        }

        // 数学公式块 $$...$$ 或 $...$
        const mathBlock = line.match(/^\$\$([^$]+)\$\$$/);
        if (mathBlock) {
            out.push('<div class="math-block">' + escapeHtml(mathBlock[1]) + '</div>');
            i++;
            continue;
        }
        const mathInline = line.match(/\$([^$]+)\$/);
        if (mathInline) {
            out.push(inlineFormat(line.replace(/\$([^$]+)\$/g, '<code class="math">$1</code>')));
            i++;
            continue;
        }

        // 表格 | a | b |（含分隔行 | --- | --- |）
        if (line.startsWith('|') && lines[i + 1] && /^\|[\s:|-]+\|/.test(lines[i + 1])) {
            const rows = [];
            while (i < lines.length && lines[i].startsWith('|')) {
                const cells = lines[i]
                    .trim()
                    .replace(/^\||\|$/g, '')
                    .split('|')
                    .map((c) => c.trim());
                rows.push(cells);
                i++;
            }
            if (rows.length > 1) {
                // 第二行是分隔行（| --- | --- |），跳过
                const header = rows[0];
                const body = rows.slice(2);
                let html = '<table><thead><tr>';
                header.forEach((c) => { html += '<th>' + inlineFormat(c) + '</th>'; });
                html += '</tr></thead><tbody>';
                body.forEach((row) => {
                    html += '<tr>';
                    header.forEach((_, ci) => {
                        html += '<td>' + inlineFormat(row[ci] || '') + '</td>';
                    });
                    html += '</tr>';
                });
                html += '</tbody></table>';
                out.push(html);
            }
            continue;
        }

        // 空行
        if (line.trim() === '') {
            if (out.length && !out[out.length - 1].endsWith('<br>')) {
                out.push('<br>');
            }
            i++;
            continue;
        }

        // 普通段落
        out.push('<p>' + inlineFormat(line) + '</p>');
        i++;
    }

    return out.join('\n');
}

// ── 剪贴板工具 ──

/**
 * 复制文本到剪贴板。
 * @param {string} text
 * @returns {Promise<void>}
 */
export function copyTextToClipboard(text) {
    if (navigator.clipboard && window.isSecureContext) {
        return navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
    }
    return Promise.resolve(fallbackCopy(text));
}

/**
 * 回退复制方案（使用隐藏 textarea）。
 * @param {string} text
 */
export function fallbackCopy(text) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    try {
        document.execCommand('copy');
    } catch (e) {
        console.error('复制失败:', e);
    }
    document.body.removeChild(ta);
}

/**
 * 复制代码块内容（从按钮向上查找 .code-block）。
 * @param {HTMLButtonElement} btn
 */
export function copyCode(btn) {
    const block = btn.closest('.code-block');
    if (!block) return;
    const code = block.querySelector('pre code');
    const text = code ? code.innerText : '';
    copyTextToClipboard(text).then(() => {
        btn.textContent = '✅ 已复制';
        setTimeout(() => { btn.textContent = '📋 复制'; }, 1500);
    });
}

/**
 * 展开/折叠长代码块。
 * @param {HTMLButtonElement} btn
 */
export function toggleCodeBlock(btn) {
    const block = btn.closest('.code-block');
    if (!block) return;
    const collapsed = block.classList.toggle('code-block-collapsed');
    btn.textContent = collapsed ? '展开' : '收起';
}

/**
 * 运行 JavaScript 代码块（P5）。
 * @param {HTMLButtonElement} btn
 */
export function runCode(btn) {
    // F1: 安全确认门 —— 阻止误触执行未知代码
    if (!confirm('确认执行这段代码？')) return;

    const block = btn.closest('.code-block');
    if (!block) return;
    const codeEl = block.querySelector('pre code');
    if (!codeEl) return;
    
    const code = codeEl.textContent || '';
    const outputEl = document.createElement('div');
    outputEl.className = 'code-run-output';
    
    try {
        const result = eval(code);
        outputEl.textContent = typeof result !== 'undefined' ? String(result) : 'undefined';
        outputEl.className += ' code-run-success';
    } catch (e) {
        outputEl.textContent = '错误: ' + e.message;
        outputEl.className += ' code-run-error';
    }
    
    // 移除旧输出
    const oldOutput = block.querySelector('.code-run-output');
    if (oldOutput) oldOutput.remove();
    
    block.appendChild(outputEl);
}

// ── LCS Diff ──

/**
 * 计算两段文本的行级 diff（LCS 算法）。
 * @param {string[]} oldLines
 * @param {string[]} newLines
 * @returns {{ lines: Array<{type: string, marker: string, text: string}>, stats: {add: number, del: number, ctx: number} }}
 */
export function lcsDiff(oldLines, newLines) {
    const n = oldLines.length;
    const m = newLines.length;
    // dp[i][j] = oldLines[i..] 与 newLines[j..] 的 LCS 长度
    const dp = [];
    for (let i = 0; i <= n; i++) {
        dp.push(new Array(m + 1).fill(0));
    }
    for (let i = n - 1; i >= 0; i--) {
        for (let j = m - 1; j >= 0; j--) {
            if (oldLines[i] === newLines[j]) {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1]);
            }
        }
    }

    const lines = [];
    let i = 0;
    let j = 0;
    let add = 0;
    let del = 0;
    let ctx = 0;
    while (i < n && j < m) {
        if (oldLines[i] === newLines[j]) {
            lines.push({ type: 'ctx', marker: ' ', text: oldLines[i] });
            ctx++;
            i++;
            j++;
        } else if (dp[i + 1][j] >= dp[i][j + 1]) {
            lines.push({ type: 'del', marker: '-', text: oldLines[i] });
            del++;
            i++;
        } else {
            lines.push({ type: 'add', marker: '+', text: newLines[j] });
            add++;
            j++;
        }
    }
    while (i < n) {
        lines.push({ type: 'del', marker: '-', text: oldLines[i] });
        del++;
        i++;
    }
    while (j < m) {
        lines.push({ type: 'add', marker: '+', text: newLines[j] });
        add++;
        j++;
    }

    return { lines, stats: { add, del, ctx } };
}

// ── 全局挂载（供 HTML onclick 调用） ──

window._daCopyCode = copyCode;
window._daToggleCodeBlock = toggleCodeBlock;
