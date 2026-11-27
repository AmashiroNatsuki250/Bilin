<script setup lang="ts">
import { computed, watch, onMounted, ref } from 'vue';
import type { Mode, ServiceMode, RemoteMode, NormalMode } from '~/composables/mode';
import { appConfigDir, appLogDir } from '@tauri-apps/api/path';
import { join } from '@tauri-apps/api/path';
import { getServiceStatus, type ServiceStatus } from '~/composables/backend';

const { t } = useI18n()

const model = defineModel<Mode>({ required: true })
const emit = defineEmits(['uninstall-service', 'stop-service'])

const defaultConfigDir = ref('')
const defaultLogDir = ref('')
const serviceStatus = ref<ServiceStatus>('NotInstalled')
const isServiceStatusLoaded = ref(false)

function normalizeRpcListenPort(port: unknown): number {
  const defaultPort = 15999
  const numericPort = typeof port === 'number' ? port : Number.parseInt(String(port ?? ''), 10)
  if (Number.isNaN(numericPort))
    return defaultPort
  return Math.min(65535, Math.max(1, Math.floor(numericPort)))
}

onMounted(async () => {
  defaultConfigDir.value = await join(await appConfigDir(), 'config.d')
  defaultLogDir.value = await appLogDir()

  // the mode watch may have run before these defaults resolved (e.g. when the
  // dialog is opened with service mode preselected), leaving the fields empty.
  if (model.value.mode === 'service') {
    const serviceModel = model.value as ServiceMode
    serviceModel.config_dir = serviceModel.config_dir || defaultConfigDir.value
    serviceModel.file_log_dir = serviceModel.file_log_dir || defaultLogDir.value
  }
})

const modeOptions = computed(() => [
  { label: t('mode.normal'), value: 'normal' },
  { label: t('mode.service'), value: 'service' },
  { label: t('mode.remote'), value: 'remote' },
]);

const normalMode = computed({
  get: () => model.value.mode === 'normal' ? model.value as NormalMode : undefined,
  set: (value) => {
    if (value) {
      model.value = value
    }
  }
})

const rpcListenOptions = computed(() => [
  { label: t('web.common.disable'), value: false },
  { label: t('web.common.enable'), value: true },
])

const rpcListenEnabled = computed<boolean>({
  get: () => !!normalMode.value?.enable_rpc_port_listen,
  set: (value) => {
    if (!normalMode.value)
      return
    normalMode.value.enable_rpc_port_listen = value
  },
})

const rpcListenPort = computed<string>({
  get: () => String(normalMode.value?.rpc_listen_port ?? 15999),
  set: (value) => {
    if (!normalMode.value)
      return
    const trimmed = value.trim()
    if (trimmed === '')
      return
    if (!/^\d+$/.test(trimmed))
      return
    normalMode.value.rpc_listen_port = Number.parseInt(trimmed, 10)
  },
})

const serviceMode = computed({
  get: () => model.value.mode === 'service' ? model.value as ServiceMode : undefined,
  set: (value) => {
    if (value) {
      model.value = value
    }
  }
})

const remoteMode = computed({
  get: () => model.value.mode === 'remote' ? model.value as RemoteMode : undefined,
  set: (value) => {
    if (value) {
      model.value = value
    }
  }
})

const logLevels = computed(() => ['off', 'warn', 'info', 'debug', 'trace'].map(value => ({
  value,
  label: t(`logging_level_${value}`),
})))

watch(() => [normalMode.value?.enable_rpc_port_listen, normalMode.value?.rpc_listen_port], ([enabled, port]) => {
  if (!normalMode.value)
    return

  if (!enabled) {
    normalMode.value.rpc_portal = undefined
    return
  }

  const normalizedPort = normalizeRpcListenPort(port)
  if (normalMode.value.rpc_listen_port !== normalizedPort)
    normalMode.value.rpc_listen_port = normalizedPort

  const desiredPortal = `tcp://0.0.0.0:${normalizedPort}`
  if (normalMode.value.rpc_portal !== desiredPortal)
    normalMode.value.rpc_portal = desiredPortal
}, { immediate: true })

watch(() => model.value.mode, async (newMode, oldMode) => {
  if (newMode === oldMode)
    return

  if (newMode === 'service' && !isServiceStatusLoaded.value) {
    serviceStatus.value = await getServiceStatus()
    isServiceStatusLoaded.value = true
  }

  const oldModelValue = { ...model.value }

  if (newMode === 'normal') {
    const portal = normalMode.value?.rpc_portal?.trim()
    model.value = {
      ...oldModelValue,
      rpc_portal: portal || undefined,
      enable_rpc_port_listen: normalMode.value?.enable_rpc_port_listen,
      rpc_listen_port: normalMode.value?.rpc_listen_port,
      mode: 'normal',
    }
  }
  else if (newMode === 'service') {
    model.value = {
      ...oldModelValue,
      mode: 'service',
      config_dir: serviceMode.value?.config_dir || defaultConfigDir.value,
      rpc_portal: serviceMode.value?.rpc_portal || '127.0.0.1:15999',
      file_log_level: serviceMode.value?.file_log_level || 'off',
      file_log_dir: serviceMode.value?.file_log_dir || defaultLogDir.value,
    }
  }
  else if (newMode === 'remote') {
    model.value = {
      ...oldModelValue,
      mode: 'remote',
      remote_rpc_address: remoteMode.value?.remote_rpc_address || 'tcp://127.0.0.1:15999',
    }
  }
}, { immediate: true })

</script>

<template>
  <div class="mode-layout">
    <SelectButton id="mode-select" v-model="model.mode" :options="modeOptions" option-label="label"
      option-value="value" :allow-empty="false" fluid />
    <p class="mode-description">{{ t('mode.' + model.mode + '_description') }}</p>

    <div v-if="normalMode" class="mode-fields">
      <div class="mode-field">
        <label for="rpc-listen-toggle">{{ t('mode.enable_rpc_tcp_listen') }}</label>
        <SelectButton id="rpc-listen-toggle" v-model="rpcListenEnabled" :options="rpcListenOptions"
          option-label="label" option-value="value" :allow-empty="false" />
      </div>
      <div v-if="rpcListenEnabled" class="mode-field">
        <label for="rpc-listen-port">{{ t('mode.rpc_listen_port') }}</label>
        <InputText id="rpc-listen-port" v-model="rpcListenPort" inputmode="numeric" />
      </div>
    </div>

    <div v-if="serviceMode" class="mode-fields">
      <div class="mode-field">
        <label for="config-dir">{{ t('mode.config_dir') }}</label>
        <InputText id="config-dir" v-model="serviceMode.config_dir" />
      </div>
      <div class="mode-field">
        <label for="rpc-portal">{{ t('mode.rpc_portal') }}</label>
        <InputText id="rpc-portal" v-model="serviceMode.rpc_portal" />
      </div>
      <div class="mode-field">
        <label for="log-dir">{{ t('mode.log_dir') }}</label>
        <InputText id="log-dir" v-model="serviceMode.file_log_dir" />
      </div>
      <div class="mode-field">
        <label for="log-level">{{ t('mode.log_level') }}</label>
        <Select input-id="log-level" v-model="serviceMode.file_log_level" :options="logLevels"
          option-label="label" option-value="value" />
      </div>
      <div class="mode-service">
        <div class="mode-field">
          <span>{{ t('mode.service_status') }}</span>
          <strong>{{ t(`mode.service_status_${serviceStatus.toLowerCase()}`) }}</strong>
        </div>
        <div class="mode-service-actions">
          <Button v-if="serviceStatus === 'Running'" :label="t('mode.stop_service')" icon="pi pi-stop-circle"
            @click="emit('stop-service')" outlined />
          <Button v-if="serviceStatus !== 'NotInstalled'" :label="t('mode.uninstall_service')" icon="pi pi-trash"
            @click="emit('uninstall-service')" outlined />
        </div>
      </div>
    </div>

    <div v-if="remoteMode" class="mode-field">
      <label for="remote-addr">{{ t('mode.remote_rpc_address') }}</label>
      <InputText id="remote-addr" v-model="remoteMode.remote_rpc_address" />
    </div>
  </div>
</template>

<style scoped>
.mode-layout, .mode-fields { display: grid; gap: 14px; }
.mode-description { color: #595959; font-size: 12px; margin: 0; }
.mode-fields { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.mode-field { display: grid; gap: 5px; min-width: 0; }
.mode-field > label, .mode-field > span { font-size: 12px; color: #555; }
.mode-field :deep(.p-inputtext), .mode-field :deep(.p-select) { min-width: 0; width: 100%; }
.mode-service { grid-column: 1 / -1; display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px; background: #f4f4f4; border-radius: 7px; }
.mode-service-actions { display: flex; flex-wrap: wrap; gap: 8px; }
@media (max-width: 450px) {
  .mode-fields { grid-template-columns: minmax(0, 1fr); }
  .mode-service { flex-wrap: wrap; }
}
</style>
