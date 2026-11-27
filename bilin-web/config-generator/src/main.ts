import { createApp } from 'vue'
import Aura from '@primeuix/themes/aura'
import BilinFrontendLib from 'bilin-frontend-lib'
import 'bilin-frontend-lib/style.css'
import PrimeVue from 'primevue/config'
import App from './App.vue'

createApp(App)
  .use(PrimeVue, {
    theme: {
      preset: Aura,
      options: {
        prefix: 'p',
        darkModeSelector: 'system',
        cssLayer: {
          name: 'primevue',
          order: 'tailwind-base, primevue, tailwind-utilities',
        },
      },
    },
  })
  .use(BilinFrontendLib)
  .mount('#app')
