// ============================================================
// Dev-Assistant Web UI — 单元测试：i18n 国际化
// ============================================================
// 运行：bun test src/web/tests/i18n.test.js

import { describe, test, expect, beforeAll } from 'bun:test';

// mock document（i18n.js 在导入时注册 Alpine store）
globalThis.window = globalThis;
globalThis.document = {
    addEventListener: (event, handler) => {
        if (event === 'alpine:init') {
            // 模拟 Alpine 环境
            globalThis.window.Alpine = {
                store: (name, obj) => {
                    globalThis.window._alpineStores = globalThis.window._alpineStores || {};
                    globalThis.window._alpineStores[name] = obj;
                },
            };
            handler();
        }
    },
};

const { i18n } = await import('../static/js/i18n.js');

// 获取 Alpine store 中的 i18n store
function getAlpineStore() {
    return globalThis.window._alpineStores?.i18n;
}

describe('I18n Alpine Store', () => {
    test('默认语言为中文', () => {
        const store = getAlpineStore();
        expect(store).toBeDefined();
        expect(store.currentLang).toBe('zh');
    });

    test('中文翻译属性正确', () => {
        const store = getAlpineStore();
        expect(store.send).toBe('发送');
        expect(store.new_chat).toBe('新对话');
        expect(store.online).toBe('在线');
        expect(store.search).toBe('搜索');
        expect(store.stop).toBe('停止');
    });

    test('切换为英文后翻译属性更新', () => {
        const store = getAlpineStore();
        store.setLanguage('en');
        expect(store.currentLang).toBe('en');
        expect(store.send).toBe('Send');
        expect(store.new_chat).toBe('New Chat');
        expect(store.online).toBe('Online');
        expect(store.stop).toBe('Stop');
    });

    test('切换回中文后翻译属性恢复', () => {
        const store = getAlpineStore();
        store.setLanguage('zh');
        expect(store.currentLang).toBe('zh');
        expect(store.send).toBe('发送');
    });

    test('toggle 切换语言', () => {
        const store = getAlpineStore();
        store.setLanguage('zh');
        store.toggle();
        expect(store.currentLang).toBe('en');
        expect(store.send).toBe('Send');
        store.toggle();
        expect(store.currentLang).toBe('zh');
        expect(store.send).toBe('发送');
    });

    test('setLanguage 无效语言不改变', () => {
        const store = getAlpineStore();
        store.setLanguage('zh');
        store.setLanguage('invalid');
        expect(store.currentLang).toBe('zh');
    });

    test('availableLanguages 返回正确', () => {
        const store = getAlpineStore();
        const langs = store.availableLanguages;
        expect(langs.length).toBe(2);
        expect(langs[0].code).toBe('zh');
        expect(langs[1].code).toBe('en');
    });
});