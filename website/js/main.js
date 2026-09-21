'use strict';

/* ======================= 主题切换 ======================= */
(function initTheme() {
  const root = document.documentElement;
  const stored = localStorage.getItem('dev-assistant-site-theme') || 'dark';
  root.setAttribute('data-theme', stored);

  const btn = document.getElementById('themeToggle');
  if (!btn) return;
  btn.addEventListener('click', () => {
    const cur = root.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', cur);
    localStorage.setItem('dev-assistant-site-theme', cur);
  });
})();

/* ======================= 导航栏滚动状态 ======================= */
(function initNavScroll() {
  const nav = document.getElementById('nav');
  if (!nav) return;
  const onScroll = () => {
    nav.classList.toggle('scrolled', window.scrollY > 10);
  };
  window.addEventListener('scroll', onScroll, { passive: true });
  onScroll();
})();

/* ======================= 移动端汉堡菜单 ======================= */
(function initBurger() {
  const burger = document.getElementById('navBurger');
  const links = document.querySelector('.nav-links');
  if (!burger || !links) return;
  burger.addEventListener('click', () => {
    const open = links.classList.toggle('open');
    burger.classList.toggle('open', open);
    burger.setAttribute('aria-expanded', open ? 'true' : 'false');
    document.body.classList.toggle('no-scroll', open);
  });
  // 点击链接后收起菜单
  links.querySelectorAll('a').forEach((a) => {
    a.addEventListener('click', () => {
      links.classList.remove('open');
      burger.classList.remove('open');
      document.body.classList.remove('no-scroll');
      burger.setAttribute('aria-expanded', 'false');
    });
  });
})();

/* ======================= 平滑滚动（锚点） ======================= */
(function initSmoothScroll() {
  document.querySelectorAll('a[href^="#"]').forEach((a) => {
    a.addEventListener('click', (e) => {
      const id = a.getAttribute('href');
      if (id === '#') return;
      const target = document.querySelector(id);
      if (!target) return;
      e.preventDefault();
      target.scrollIntoView({ behavior: 'smooth', block: 'start' });
    });
  });
})();

/* ======================= 滚动显现动画（IntersectionObserver） ======================= */
(function initReveal() {
  const els = document.querySelectorAll('.feature-card, .step, .arch-card, .cmd-group, .stage');
  if (!('IntersectionObserver' in window)) {
    els.forEach((el) => el.classList.add('revealed'));
    return;
  }
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add('revealed');
          io.unobserve(entry.target);
        }
      });
    },
    { threshold: 0.12, rootMargin: '0px 0px -40px 0px' },
  );
  els.forEach((el) => io.observe(el));
})();

/* ======================= 终端打字动画 ======================= */
(function initTerminal() {
  const output = document.getElementById('termLines');
  const cursor = document.getElementById('termCursor');
  if (!output || !cursor) return;

  // 终端脚本：每一行 { text, pause }，特殊类型 type:'special'
  const script = [
    { type: 'cmd', text: '$ dev-assistant --web' },
    { type: 'out', text: '🚀 Dev-Assistant Router 已启动 → http://127.0.0.1:8080' },
    { type: 'out', text: '' },
    { type: 'cmd', text: '> 审查一下当前项目的安全风险' },
    { type: 'thinking', text: '💭 正在阅读整个代码库（batch_read_files）…' },
    { type: 'tool', text: '✅ batch_read_files 完成：50/50 文件' },
    { type: 'tool', text: '🔍 read_symbol SecurityPolicy::validate 定位成功' },
    { type: 'out', text: '' },
    { type: 'assistant', text: '🤖 发现 2 个风险：' },
    { type: 'assistant', text: '   1. 命令注入：src/tools/bash.rs:42 未过滤输入' },
    { type: 'assistant', text: '   2. 路径遍历：src/tools/file/io.rs:88 缺少规范化' },
    { type: 'out', text: '' },
    { type: 'cmd', text: '> 自动修复并补测试' },
    { type: 'thinking', text: '💭 启动 6 阶段流水线 /pipeline …' },
    { type: 'tool', text: '🏗 架构设计 → 💻 代码实现 → 🧪 测试验证 → 🔍 审查 → 🔧 修复 → 📋 记录' },
    { type: 'assistant', text: '✅ 已修复 2 个问题，cargo test 全部通过 (42/42)' },
  ];

  const code = output.querySelector('code') || output;
  let lineIdx = 0;
  let charIdx = 0;
  let busy = false;

  // 是否应跳过动画（用户偏好减少动态 / 触屏长动画频繁重跑）
  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  let allowStart = true;

  function typeLine(el, text, speed, done) {
    if (reduceMotion) {
      el.textContent = text;
      done();
      return;
    }
    charIdx = 0;
    const tick = () => {
      if (charIdx < text.length) {
        charIdx += 1;
        el.textContent = text.slice(0, charIdx);
        setTimeout(tick, speed);
      } else {
        done();
      }
    };
    tick();
  }

  function skipAll() {
    // 直接渲染剩余全部行
    let remaining = script.slice(lineIdx);
    remaining.forEach((line) => {
      const el = document.createElement('div');
      el.className = 'term-' + line.type;
      el.textContent = line.text;
      code.appendChild(el);
    });
    lineIdx = script.length;
    cursor.style.display = 'none';
  }

  function next() {
    if (busy || lineIdx >= script.length) {
      // 动画结束
      if (cursor) cursor.style.display = 'none';
      return;
    }
    const line = script[lineIdx];
    lineIdx += 1;
    const el = document.createElement('div');
    el.className = 'term-' + line.type;
    code.appendChild(el);
    busy = true;

    const done = () => {
      busy = false;
      // 行与行之间的小停顿
      setTimeout(next, line.pause || (line.type === 'cmd' ? 420 : 120));
    };

    if (line.text === '') {
      busy = false;
      setTimeout(next, 80);
      return;
    }

    // 命令/思考行整行闪烁后打字，其他行直接打字
    if (reduceMotion) {
      el.textContent = line.text;
      done();
      return;
    }
    typeLine(el, line.text, line.type === 'cmd' ? 34 : 16, done);
  }

  // 点击终端立即跳过动画
  const skip = () => skipAll();
  output.closest('.terminal')?.addEventListener('click', (e) => {
    if (lineIdx < script.length && e.target !== output) skipAll();
  });

  // 可见才启动（30% 可见即开始）
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting && allowStart) {
          allowStart = false;
          io.disconnect();
          setTimeout(next, 400);
        }
      });
    },
    { threshold: 0.25 },
  );
  io.observe(output.closest('.terminal') || output);
})();

/* ======================= 统计数字滚动 ======================= */
(function initCounters() {
  const nums = document.querySelectorAll('.stat-num');
  if (!nums.length) return;
  const fmt = (n) => n;
  const animate = (el, val) => {
    if (val === 0) {
      el.textContent = '0';
      return;
    }
    const duration = 900;
    const start = performance.now();
    const tick = (now) => {
      const p = Math.min((now - start) / duration, 1);
      const eased = 1 - Math.pow(1 - p, 3);
      el.textContent = fmt(Math.round(val * eased));
      if (p < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        const el = entry.target;
        const text = el.textContent.trim();
        const num = parseInt(text, 10);
        // 非数字统计（∞ / ≤3 等）保持原文本，不执行计数动画
        if (isNaN(num)) {
          io.unobserve(el);
          return;
        }
        animate(el, num);
        io.unobserve(el);
      });
    },
    { threshold: 0.4 },
  );
  nums.forEach((n) => io.observe(n));
})();