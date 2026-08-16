// ============================================================
// Dev-Assistant Web UI — Service Worker（离线缓存）
// ============================================================
// 功能：缓存静态资源，支持离线访问

// E1: 缓存版本。每次静态资源发生破坏性变更（文件改名/结构变动）后，
// 必须递增此版本以触发旧缓存清理；同时下方 message 通道允许页面在
// 检测到新 SW 时主动激活，降低"忘记 bump 导致用户拿到旧资源"的风险。
const CACHE_NAME = 'dev-assistant-v3';
const STATIC_ASSETS = [
    '/',
    '/static/css/app.css',
    '/static/js/theme.js',
    '/static/js/utils.js',
    '/static/js/i18n.js',
    '/static/js/sidebar.js',
    '/static/js/chat.js',
    '/static/js/file-explorer.js',
];

// 安装：预缓存静态资源
self.addEventListener('install', (event) => {
    event.waitUntil(
        caches.open(CACHE_NAME)
            .then((cache) => {
                console.log('[SW] 预缓存静态资源');
                return cache.addAll(STATIC_ASSETS);
            })
            .then(() => self.skipWaiting())
    );
});

// 激活：清理旧缓存
self.addEventListener('activate', (event) => {
    event.waitUntil(
        caches.keys()
            .then((keys) => {
                return Promise.all(
                    keys
                        .filter((key) => key !== CACHE_NAME)
                        .map((key) => caches.delete(key))
                );
            })
            .then(() => self.clients.claim())
    );
});

// E1: 页面 → SW 消息通道。收到 {type:'SKIP_WAITING'} 时立即接管，
// 配合注册端 controllerchange 监听实现"有更新即生效"。
self.addEventListener('message', (event) => {
    if (event.data && event.data.type === 'SKIP_WAITING') {
        self.skipWaiting();
    }
});

// 请求拦截：缓存优先策略
self.addEventListener('fetch', (event) => {
    const { request } = event;
    const url = new URL(request.url);

    // 只缓存同源请求
    if (url.origin !== self.location.origin) {
        return;
    }

    // 静态资源：缓存优先
    if (request.method === 'GET' && (
        url.pathname.startsWith('/static/') ||
        url.pathname.endsWith('.css') ||
        url.pathname.endsWith('.js') ||
        url.pathname.endsWith('.png') ||
        url.pathname.endsWith('.svg') ||
        url.pathname.endsWith('.ico')
    )) {
        event.respondWith(
            caches.match(request)
                .then((cached) => {
                    if (cached) {
                        return cached;
                    }
                    return fetch(request)
                        .then((response) => {
                            if (response.ok) {
                                const clone = response.clone();
                                caches.open(CACHE_NAME)
                                    .then((cache) => cache.put(request, clone));
                            }
                            return response;
                        });
                })
        );
        return;
    }

    // API 请求：网络优先
    if (url.pathname.startsWith('/api/')) {
        event.respondWith(
            fetch(request)
                .catch(() => caches.match(request))
        );
        return;
    }

    // HTML 页面：网络优先，失败时返回缓存
    if (request.headers.get('accept')?.includes('text/html')) {
        event.respondWith(
            fetch(request)
                .then((response) => {
                    if (response.ok) {
                        const clone = response.clone();
                        caches.open(CACHE_NAME)
                            .then((cache) => cache.put(request, clone));
                    }
                    return response;
                })
                .catch(() => caches.match(request))
        );
        return;
    }
});
