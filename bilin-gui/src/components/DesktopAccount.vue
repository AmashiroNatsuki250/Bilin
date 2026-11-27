<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue'

type Mode = 'login' | 'register'

const props = defineProps<{
  busy?: boolean
  error?: string
  captchaImage?: string
  captchaLoading?: boolean
}>()

const emit = defineEmits<{
  signIn: [username: string, password: string]
  register: [username: string, password: string, captcha: string]
  refreshCaptcha: []
  clearError: []
}>()

const id = useId()
const mode = ref<Mode>('login')
const loginTab = ref<HTMLButtonElement>()
const registerTab = ref<HTMLButtonElement>()
const username = ref('')
const password = ref('')
const passwordVisible = ref(false)
const captcha = ref('')

const registering = computed(() => mode.value === 'register')
const canSubmit = computed(() => Boolean(
  !props.busy && username.value.trim() && password.value
  && (!registering.value || (props.captchaImage && captcha.value.trim() && !props.captchaLoading)),
))

watch(() => props.captchaImage, () => { captcha.value = '' })

function refreshCaptcha() {
  if (props.busy || props.captchaLoading) return
  captcha.value = ''
  emit('clearError')
  emit('refreshCaptcha')
}

function chooseMode(next: Mode) {
  if (props.busy || next === mode.value) return
  mode.value = next
  password.value = ''
  passwordVisible.value = false
  captcha.value = ''
  if (next === 'register' && !props.captchaLoading) refreshCaptcha()
  else emit('clearError')
}

function onTabKey(event: KeyboardEvent) {
  if (props.busy) return
  const current = event.target
  let next: HTMLButtonElement | undefined
  switch (event.key) {
    case 'ArrowLeft':
    case 'ArrowRight':
      next = current === loginTab.value ? registerTab.value : loginTab.value
      break
    case 'Home':
      next = loginTab.value
      break
    case 'End':
      next = registerTab.value
      break
    default:
      return
  }
  event.preventDefault()
  next?.focus()
}

function submit() {
  if (!canSubmit.value) return
  const name = username.value.trim()
  const secret = password.value
  const challenge = captcha.value.trim()
  password.value = ''
  passwordVisible.value = false
  captcha.value = ''
  emit('clearError')
  if (registering.value) emit('register', name, secret, challenge)
  else emit('signIn', name, secret)
}
</script>

<template>
  <main class="desktop-account" :aria-labelledby="`${id}-title`">
    <section class="account-card">
      <header class="account-hero">
        <h1 :id="`${id}-title`">{{ registering ? '创建比邻账号' : '登录比邻' }}</h1>
        <p>{{ registering ? '同一账号可用于 Web 控制台' : '使用 Web 控制台账号' }}</p>
      </header>

      <div class="account-content">
        <div class="account-tabs" role="tablist" aria-label="账号操作" @keydown="onTabKey">
          <button :id="`${id}-login-tab`" ref="loginTab" type="button" role="tab" :aria-selected="!registering"
            :aria-controls="`${id}-form`" :tabindex="registering ? -1 : 0" :disabled="busy" @click="chooseMode('login')">
            登录
          </button>
          <button :id="`${id}-register-tab`" ref="registerTab" type="button" role="tab" :aria-selected="registering"
            :aria-controls="`${id}-form`" :tabindex="registering ? 0 : -1" :disabled="busy" @click="chooseMode('register')">
            注册账号
          </button>
        </div>

        <div :id="`${id}-form`" role="tabpanel" :aria-labelledby="`${id}-${mode}-tab`">
          <form class="account-form" :aria-busy="busy || captchaLoading" @submit.prevent="submit">
            <div class="account-field">
              <label :for="`${id}-username`">用户名</label>
              <input :id="`${id}-username`" v-model="username" :placeholder="registering ? '设置用户名' : '输入用户名'"
                autocomplete="username" autocapitalize="none" spellcheck="false" required :disabled="busy" />
            </div>

            <div class="account-field">
              <label :for="`${id}-password`">密码</label>
              <div class="account-password">
                <input :id="`${id}-password`" v-model="password" :type="passwordVisible ? 'text' : 'password'"
                  :placeholder="registering ? '设置密码' : '输入密码'" :autocomplete="registering ? 'new-password' : 'current-password'"
                  spellcheck="false" required :disabled="busy" />
                <button type="button" class="account-eye" :aria-label="passwordVisible ? '隐藏密码' : '显示密码'"
                  :disabled="busy" @click="passwordVisible = !passwordVisible">
                  <i :class="passwordVisible ? 'pi pi-eye-slash' : 'pi pi-eye'" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div v-if="registering" class="account-field">
              <label :for="`${id}-captcha`">验证码</label>
              <div class="account-captcha">
                <input :id="`${id}-captcha`" v-model="captcha" placeholder="输入图形验证码" autocomplete="off"
                  autocapitalize="off" spellcheck="false" required :disabled="busy || captchaLoading" />
                <button type="button" aria-label="更换验证码" :disabled="busy || captchaLoading" @click="refreshCaptcha">
                  <img v-if="captchaImage && !captchaLoading" :src="captchaImage" alt="图形验证码" />
                  <i :class="captchaLoading ? 'pi pi-spin pi-spinner' : 'pi pi-refresh'" aria-hidden="true" />
                </button>
              </div>
            </div>

            <p v-if="error" class="account-error" role="alert">{{ error }}</p>

            <button type="submit" class="account-submit" :disabled="!canSubmit">
              <i v-if="busy" class="pi pi-spin pi-spinner" aria-hidden="true" />
              {{ busy ? (registering ? '正在创建账号…' : '正在登录…') : (registering ? '注册并登录' : '登录并绑定本机') }}
            </button>
          </form>
        </div>

        <p class="account-note">
          <i class="pi pi-shield" aria-hidden="true" />
          {{ registering ? '注册后自动登录并绑定本机' : '登录后直接进入主窗口' }}
        </p>
        <slot name="extra" :mode="mode" />
      </div>
    </section>
  </main>
</template>

<style scoped>
.desktop-account {
  box-sizing: border-box;
  display: grid;
  align-items: start;
  justify-items: center;
  height: 100dvh;
  padding: 16px 16px 20px;
  overflow: auto;
  color: #232623;
  background: #fcfdfa;
  font-family: system-ui, 'Microsoft YaHei', 'PingFang SC', sans-serif;
}

.account-card {
  width: min(100%, 420px);
  min-width: 0;
}

.account-hero {
  position: relative;
  isolation: isolate;
  overflow: hidden;
  padding: 19px 17px;
  border-radius: 12px;
  color: #f9f9f9;
  background: #242623;
}

.account-hero::before {
  position: absolute;
  z-index: -1;
  inset: 0;
  background: radial-gradient(circle, #626662 .95px, transparent 1.3px) 0 0 / 12px 12px;
  -webkit-mask-image: linear-gradient(to right, transparent 18%, #000);
  mask-image: linear-gradient(to right, transparent 18%, #000);
  opacity: .55;
  content: '';
}

.account-hero h1 { margin: 0; font-size: 26px; font-weight: 740; line-height: 1.25; }
.account-hero p { margin: 7px 0 0; color: #dde1da; font-size: 14px; line-height: 1.4; }
.account-content { padding: 16px 2px 0; }

.account-tabs {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px;
  margin-bottom: 17px;
  padding: 4px;
  border: 1px solid #e0e4de;
  border-radius: 11px;
  background: #eff2ee;
}

.account-tabs button {
  min-height: 36px;
  border: 1px solid transparent;
  border-radius: 8px;
  color: #4b544b;
  font-size: 14px;
  font-weight: 650;
}

.account-tabs button[aria-selected='true'] {
  border-color: #ddd;
  color: #222;
  background: #fff;
  box-shadow: 0 1px 2px #1a1a1a0d;
}

.account-form { display: grid; gap: 14px; }
.account-field { display: grid; min-width: 0; gap: 7px; }
.account-field label { font-size: 14px; font-weight: 690; }

.account-form input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  height: 45px;
  padding: 0 12px;
  border: 1px solid #d7ded6;
  border-radius: 9px;
  color: #222;
  background: #fcfdfa;
  font: inherit;
  font-size: 14px;
}

.account-form input::placeholder { color: #696969; opacity: 1; }
.account-password { position: relative; }
.account-password input { padding-inline-end: 40px; }

.account-eye {
  position: absolute;
  inset-block: 5px;
  inset-inline-end: 5px;
  display: grid;
  width: 34px;
  place-items: center;
  border-radius: 5px;
  color: #454545;
}

.account-captcha { display: grid; grid-template-columns: minmax(0, 1fr) 104px; gap: 8px; }

.account-captcha button {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 45px;
  padding: 2px 4px;
  border: 1px solid #ddd;
  border-radius: 7px;
  color: #454545;
  background: #f4f4f4;
}

.account-captcha img { display: block; width: 72px; height: 33px; object-fit: contain; }
.account-captcha button i { font-size: 11px; }

.account-error {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  color: #902a2a;
  background: #fff3f3;
  font-size: 12px;
  overflow-wrap: anywhere;
}

.account-submit {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 47px;
  margin-top: 2px;
  border-radius: 9px;
  color: #fff;
  background: #262925;
  font-size: 14px;
  font-weight: 650;
}

.account-submit:hover:not(:disabled) { background: #363636; }
.account-eye:hover:not(:disabled) { background: #ededed; }
.account-card :disabled { cursor: not-allowed; }
.account-submit:disabled { color: #f3f4f1; }
.account-card :is(input, button):focus-visible { outline: 2px solid #6a6a6a; outline-offset: 2px; }

.account-note {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 14px 0 0;
  color: #586358;
  font-size: 13px;
  line-height: 1.45;
}
</style>
