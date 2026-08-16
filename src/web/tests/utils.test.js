// ============================================================
// Dev-Assistant Web UI — 单元测试：utils.js 纯函数
// ============================================================
// 运行：bun test src/web/tests/utils.test.js

import { describe, test, expect, beforeAll } from 'bun:test';

// utils.js 在文件末尾引用 window，测试环境需先定义
globalThis.window = globalThis;

const { escapeHtml, escapeHtmlText, inlineFormat,
        highlightCode, renderMarkdown, lcsDiff } =
    await import('../static/js/utils.js');

describe('escapeHtml', () => {
    test('转义 HTML 特殊字符', () => {
        expect(escapeHtml('<script>alert("x&y")</script>'))
            .toBe('&lt;script&gt;alert(&quot;x&amp;y&quot;)&lt;/script&gt;');
    });

    test('空字符串返回空', () => {
        expect(escapeHtml('')).toBe('');
    });

    test('转义未排除数字', () => {
        expect(escapeHtml('a&b<c>d"e')).toBe('a&amp;b&lt;c&gt;d&quot;e');
    });

    test('escapeHtmlText 是别名', () => {
        expect(escapeHtmlText('<div>')).toBe(escapeHtml('<div>'));
    });
});

describe('inlineFormat', () => {
    test('行内代码 `x`', () => {
        expect(inlineFormat('使用 `cargo build` 编译'))
            .toContain('<code>cargo build</code>');
    });

    test('粗体 **x**', () => {
        expect(inlineFormat('这是**重要**内容'))
            .toContain('<strong>重要</strong>');
    });

    test('删除线 ~~x~~', () => {
        expect(inlineFormat('~~废弃~~保留'))
            .toContain('<del>废弃</del>');
    });

    test('链接 [text](url)', () => {
        expect(inlineFormat('访问[官网](https://example.com)'))
            .toContain('<a href="https://example.com" target="_blank" rel="noopener">官网</a>');
    });
});

describe('highlightCode', () => {
    test('生成代码块结构', () => {
        const html = highlightCode('let x = 1;', 'javascript');
        expect(html).toContain('code-block');
        expect(html).toContain('language-javascript');
        expect(html).toContain('复制');
    });

    test('JS 代码块带运行按钮', () => {
        const html = highlightCode('console.log(1)', 'js');
        expect(html).toContain('run-btn');
        expect(html).toContain('▶ 运行');
    });

    test('长代码块折叠（超 20 行）', () => {
        const longCode = Array.from({ length: 25 }, (_, i) => 'line' + i).join('\n');
        const html = highlightCode(longCode, 'python');
        expect(html).toContain('code-block-collapsed');
        expect(html).toContain('展开');
    });

    test('无语言时使用默认标签', () => {
        const html = highlightCode('plain text');
        expect(html).toContain('>code<');
    });
});

describe('renderMarkdown', () => {
    test('渲染标题', () => {
        const html = renderMarkdown('# 一级标题\n## 二级标题');
        expect(html).toContain('<h2>一级标题</h2>');
        expect(html).toContain('<h3>二级标题</h3>');
    });

    test('渲染代码块', () => {
        const html = renderMarkdown('```rust\nfn main() {}\n```');
        expect(html).toContain('code-block');
    });

    test('渲染无序列表', () => {
        const html = renderMarkdown('- 苹果\n- 香蕉');
        expect(html).toContain('<ul>');
        expect(html).toContain('<li>苹果</li>');
        expect(html).toContain('<li>香蕉</li>');
    });

    test('渲染表格', () => {
        const md = '| 名称 | 值 |\n| --- | --- |\n| A | 1 |\n| B | 2 |';
        const html = renderMarkdown(md);
        expect(html).toContain('<table>');
        expect(html).toContain('<th>名称</th>');
        expect(html).toContain('<td>1</td>');
    });

    test('渲染任务列表', () => {
        const html = renderMarkdown('- [x] 已完成\n- [ ] 待办');
        expect(html).toContain('task-item');
        expect(html).toContain('checked');
    });

    test('渲染数学公式块', () => {
        const html = renderMarkdown('$$E = mc^2$$');
        expect(html).toContain('math-block');
        expect(html).toContain('E = mc^2');
    });

    test('渲染引用', () => {
        const html = renderMarkdown('> 这是引用');
        expect(html).toContain('<blockquote>');
    });

    test('空内容返回空', () => {
        expect(renderMarkdown('')).toBe('');
    });

    test('普通段落', () => {
        const html = renderMarkdown('第一段\n第二段');
        expect(html).toContain('<p>第一段</p>');
        expect(html).toContain('<p>第二段</p>');
    });
});

describe('lcsDiff', () => {
    test('无变化', () => {
        const result = lcsDiff(['a', 'b'], ['a', 'b']);
        expect(result.stats.add).toBe(0);
        expect(result.stats.del).toBe(0);
        expect(result.stats.ctx).toBe(2);
        expect(result.lines.every(l => l.type === 'ctx')).toBe(true);
    });

    test('新增行', () => {
        const result = lcsDiff(['a'], ['a', 'b']);
        expect(result.stats.add).toBe(1);
        expect(result.lines.some(l => l.type === 'add' && l.text === 'b')).toBe(true);
    });

    test('删除行', () => {
        const result = lcsDiff(['a', 'b'], ['a']);
        expect(result.stats.del).toBe(1);
        expect(result.lines.some(l => l.type === 'del' && l.text === 'b')).toBe(true);
    });

    test('修改行（删除+新增）', () => {
        const result = lcsDiff(['a', 'b'], ['a', 'c']);
        expect(result.stats.del).toBe(1);
        expect(result.stats.add).toBe(1);
    });

    test('空数组', () => {
        const result = lcsDiff([], []);
        expect(result.stats.add).toBe(0);
        expect(result.stats.del).toBe(0);
    });
});