import { createApp } from 'vue'
import 'bilin-frontend-lib/style.css'
import './style.css'
import App from './App.vue'
import PrimeVue from 'primevue/config'
import ConsoleTheme from './theme';
import { I18nUtils } from 'bilin-frontend-lib'

import { createRouter, createWebHashHistory } from 'vue-router'
import Login from './components/Login.vue'
import ControllerWorkspace from './components/ControllerWorkspace.vue'
import ToastService from 'primevue/toastservice';

const routes = [
    { path: '/', name: 'home', component: ControllerWorkspace, props: { home: true } },
    { path: '/device-login', name: 'device-login', component: Login, props: { isRegistering: false, bindDevice: true } },
    { path: '/desktop-access', name: 'desktop-access', component: () => import('./components/HostedDesktopAccount.vue') },
    { path: '/workspace', name: 'workspace', component: ControllerWorkspace, props: { admin: false } },
    { path: '/admin', name: 'admin', component: ControllerWorkspace, props: { admin: true } },
    { path: '/admin/users', name: 'admin-users', component: ControllerWorkspace, props: { admin: true, usersPage: true } },
    {
        path: '/auth', children: [
            {
                name: 'login',
                path: '',
                component: Login,
                alias: 'login',
                props: { isRegistering: false }
            },
            {
                name: 'register',
                path: 'register',
                component: Login,
                props: { isRegistering: true }
            }
        ]
    },
    {
        path: '/:pathMatch(.*)*', redirect: { name: 'home' }
    }
]

const router = createRouter({
    history: createWebHashHistory(),
    routes,
})

const app = createApp(App)

void I18nUtils.loadLanguageAsync('cn')
app.use(I18nUtils.i18n)
app.use(PrimeVue,
    {
        locale: {
            emptyMessage: '暂无选项',
            emptySearchMessage: '没有匹配项',
            passwordPrompt: '请输入密码',
            weak: '弱',
            medium: '中',
            strong: '强',
            aria: { close: '关闭', clear: '清除', listView: '列表' }
        },
        theme: {
            preset: ConsoleTheme,
            options: {
                prefix: 'p',
                darkModeSelector: false,
                cssLayer: {
                    name: 'primevue',
                    order: 'tailwind-base, primevue, tailwind-utilities'
                }
            }
        }
    }
).use(ToastService).use(router)

app.mount('#app')
