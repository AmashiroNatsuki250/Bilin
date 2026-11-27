import Aura from '@primeuix/themes/aura';
import { definePreset } from '@primeuix/themes';
import PrimeVue from 'primevue/config';

import BilinFrontendLib, { I18nUtils } from 'bilin-frontend-lib';
import { createRouter, createWebHistory } from 'vue-router/auto';
import { routes } from 'vue-router/auto-routes';
import App from '~/App.vue';

import 'bilin-frontend-lib/style.css';
import { ConfirmationService, DialogService, ToastService } from 'primevue';
import '~/styles.css';

const monochrome = {
  0: '#fff', 50: '#fafafa', 100: '#f2f2f2', 200: '#e5e5e5', 300: '#d4d4d4', 400: '#a3a3a3',
  500: '#737373', 600: '#555', 700: '#404040', 800: '#292929', 900: '#1c1c1c', 950: '#111',
};
const DesktopTheme = definePreset(Aura, {
  semantic: {
    primary: monochrome,
    colorScheme: {
      light: {
        surface: monochrome,
        primary: { color: '#202020', hoverColor: '#393939', activeColor: '#0f0f0f', contrastColor: '#fff' },
        highlight: { background: '#eeeeee', focusBackground: '#e2e2e2', color: '#141414', focusColor: '#141414' },
      },
    },
  },
});

if (import.meta.env.PROD) {
  document.addEventListener('keydown', (event) => {
    if (
      event.key === 'F5'
      || (event.ctrlKey && event.key === 'r')
      || (event.metaKey && event.key === 'r')
    ) {
      event.preventDefault()
    }
  })

  document.addEventListener('contextmenu', (event) => {
    event.preventDefault()
  })
}

async function main() {
  await I18nUtils.loadLanguageAsync(localStorage.getItem('lang') || 'cn')

  const app = createApp(App)

  const router = createRouter({
    history: createWebHistory(),
    routes,
  })

  app.use(router)
  app.use(createPinia())
  app.use(BilinFrontendLib, { skipPrimeVue: true })
  app.use(PrimeVue, {
    theme: {
      preset: DesktopTheme,
      options: {
        prefix: 'p',
        darkModeSelector: false,
        cssLayer: {
          name: 'primevue',
          order: 'tailwind-base, primevue, tailwind-utilities',
        },
      },
    },
  })
  app.use(ToastService)
  app.use(DialogService)
  app.use(ConfirmationService)
  app.mount('#app')
}

main()
