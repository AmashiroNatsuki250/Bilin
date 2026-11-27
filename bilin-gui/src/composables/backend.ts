import { invoke } from '@tauri-apps/api/core'
import { type } from '@tauri-apps/plugin-os'
import { Api, NetworkTypes } from 'bilin-frontend-lib'
import { GetNetworkMetasResponse } from 'node_modules/bilin-frontend-lib/dist/modules/api'
import { type ConfigSource, normalizeConfigSource } from './config_source'
import type { OperationOutcome, ManagementStatus } from './management_status'

type NetworkConfig = NetworkTypes.NetworkConfig
type ValidateConfigResponse = Api.ValidateConfigResponse
type ListNetworkInstanceIdResponse = Api.ListNetworkInstanceIdResponse
interface ServiceOptions {
  config_dir: string
  rpc_portal: string
  file_log_level: string
  file_log_dir: string
  config_server?: string
}

export type ServiceStatus = "Running" | "Stopped" | "NotInstalled"

interface StoredGuiConfig {
  config: NetworkConfig
  source: ConfigSource
}

export function parseStoredConfigs(raw: string | null): StoredGuiConfig[] {
  const parsed: unknown = JSON.parse(raw || '[]')
  if (!Array.isArray(parsed)) {
    return []
  }

  return parsed.flatMap((entry): StoredGuiConfig[] => {
    if (entry && typeof entry === 'object' && 'config' in entry) {
      const { config, source } = entry as {
        config?: NetworkConfig
        source?: unknown
      }
      if (!config) {
        return []
      }
      return [{
        config: NetworkTypes.normalizeNetworkConfig(config),
        source: normalizeConfigSource(source),
      }]
    }

    return [{
      config: NetworkTypes.normalizeNetworkConfig(entry as NetworkConfig),
      source: 'legacy',
    }]
  })
}

export async function parseNetworkConfig(cfg: NetworkConfig) {
  return invoke<string>('parse_network_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function generateNetworkConfig(tomlConfig: string) {
  const config = await invoke<NetworkConfig>('generate_network_config', { tomlConfig })
  return NetworkTypes.normalizeNetworkConfig(config)
}

export async function runNetworkInstance(cfg: NetworkConfig, save: boolean) {
  return invoke<OperationOutcome>('run_network_instance', { cfg: NetworkTypes.toBackendNetworkConfig(cfg), save })
}

export async function getManagementStatus() {
  return invoke<ManagementStatus>('get_management_status')
}

export async function collectNetworkInfo(instanceId: string) {
  return await invoke<Api.CollectNetworkInfoResponse>('collect_network_info', { instanceId })
}

export async function getVpnPortalInfo(instanceId: string) {
  const info = await invoke<NetworkTypes.VpnPortalInfo | undefined>('get_vpn_portal_info', { instanceId })
  return info ? NetworkTypes.normalizeVpnPortalInfo(info) : undefined
}

export async function addVpnPortalClient(instanceId: string, client: { name: string, virtual_ip: string, groups: string[] }) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'add', name: client.name, virtualIp: client.virtual_ip, groups: client.groups })
}

export async function removeVpnPortalClient(instanceId: string, name: string) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'remove', name })
}

export async function clearVpnPortalClients(instanceId: string) {
  return invoke('patch_vpn_portal_clients', { instanceId, action: 'clear' })
}

export async function setLoggingLevel(level: string) {
  return await invoke('set_logging_level', { level })
}

export interface MobileTunConfig {
  instance_ids: string[]
  addresses: string[]
  routes: string[]
  dns?: string | null
  rejected: { instance_id: string, error: string }[]
}

export async function getMobileTunConfig(instanceIds: string[]) {
  return await invoke<MobileTunConfig>('get_mobile_tun_config', { instanceIds })
}

export async function setTunFd(fd: number, instanceIds?: string[]) {
  return await invoke<MobileTunConfig>('set_tun_fd', { fd, instanceIds })
}

export async function getBilinVersion() {
  return await invoke<string>('bilin_version')
}

export async function listNetworkInstanceIds() {
  return await invoke<ListNetworkInstanceIdResponse>('list_network_instance_ids')
}

export async function deleteNetworkInstance(instanceId: string) {
  return await invoke<OperationOutcome>('remove_network_instance', { instanceId })
}

export async function updateNetworkConfigState(instanceId: string, disabled: boolean) {
  return await invoke<OperationOutcome>('update_network_config_state', { instanceId, disabled })
}

export async function saveNetworkConfig(cfg: NetworkConfig) {
  return await invoke('save_network_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function validateConfig(cfg: NetworkConfig) {
  return await invoke<ValidateConfigResponse>('validate_config', { cfg: NetworkTypes.toBackendNetworkConfig(cfg) })
}

export async function getConfig(instanceId: string) {
  const config = await invoke<NetworkConfig>('get_config', { instanceId })
  return NetworkTypes.normalizeNetworkConfig(config)
}

export async function sendConfigs(enabledNetworks: string[]) {
  const networkList = type() === 'android' ? [] : parseStoredConfigs(localStorage.getItem('networkList'))
  return await invoke('load_configs', {
    configs: networkList.map(({ config, source }) => ({
      config: NetworkTypes.toBackendNetworkConfig(config),
      source,
    })),
    enabledNetworks,
  })
}

export async function getNetworkMetas(instanceIds: string[]) {
  return await invoke<GetNetworkMetasResponse>('get_network_metas', { instanceIds })
}

export async function initService(opts?: ServiceOptions) {
  return await invoke('init_service', { opts })
}

export async function setServiceStatus(enable: boolean) {
  return await invoke('set_service_status', { enable })
}

export async function getServiceStatus() {
  return await invoke<ServiceStatus>('get_service_status')
}

export async function initRpcConnection(isNormalMode: boolean, url?: string) {
  return await invoke('init_rpc_connection', { isNormalMode, url })
}

export async function isClientRunning() {
  return await invoke<boolean>('is_client_running')
}

export async function initWebClient(url?: string, subnetProxyEnabled?: boolean, desktopTunnelEnabled?: boolean) {
  return await invoke('init_web_client', { url, subnetProxyEnabled, desktopTunnelEnabled })
}

export function setManagedSubnetProxyEnabled(enabled: boolean) {
  return invoke<void>('set_managed_subnet_proxy_enabled', { enabled })
}

export interface ControllerEnrollmentStatus {
  device_id: string
  controller_origin: string
  device_code?: string | null
  device_name?: string | null
  device_code_claim_supported?: boolean
  device_login_supported?: boolean
  device_login_revision?: number | null
  bound_account?: string | null
  revoked?: boolean
  account_suspended?: boolean
  registered: boolean
  bound: boolean
  connected: boolean
  desktop_tunnel_enabled?: boolean
  pairing_code?: string | null
  networks: { network_id: string; display_name: string }[]
}

export function getControllerEnrollmentStatus() {
  return invoke<ControllerEnrollmentStatus | null>('controller_enrollment_status')
}

export function setManagedTunnelEnabled(
  enabled: boolean,
  enrollment: Pick<ControllerEnrollmentStatus, 'controller_origin' | 'device_id' | 'device_login_revision'>,
) {
  if (!enrollment.device_login_revision) return Promise.reject('登录状态尚未确认，请稍后重试。')
  return invoke<void>('set_managed_tunnel_enabled', {
    enabled,
    controllerOrigin: enrollment.controller_origin,
    deviceId: enrollment.device_id,
    revision: enrollment.device_login_revision,
  })
}

export function isManagedDeviceName(name: string) {
  const trimmed = name.trim()
  return trimmed.length > 0 && [...trimmed].length <= 32 && !/\p{Cc}/u.test(trimmed)
}

export function getManagedDeviceName() {
  return invoke<string>('get_managed_device_name')
}

export function renameCurrentDevice(name: string) {
  return invoke<string>('rename_current_device', { name })
}

export interface DeviceLoginLaunch {
  revision: number
  login_url: string
  browser_opened: boolean
}

export function openControllerDeviceLogin() {
  return invoke<DeviceLoginLaunch>('open_controller_device_login')
}

export async function isWebClientConnected() {
  return await invoke<boolean>('is_web_client_connected')
}
