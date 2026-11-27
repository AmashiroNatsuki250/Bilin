<script setup lang="ts">
import { useTimeAgo } from '@vueuse/core'
import { isRetiredTransport, NetworkInstance, type DesktopPanelPeer, type NetworkConfig, type TunnelInfo, type NodeInfo, type PeerRoutePair } from '../types/network'
import type { RemoteClient } from '../modules/api'
import { useI18n } from 'vue-i18n';
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { ipv4InetToString, ipv4ToString, ipv6ToString } from '../modules/utils';
import { latencyMs, lossRate, numericValue, peerConns, preferredConn, udpNatLevel, udpNatTypeName } from '../modules/statusDisplay';
import { Badge, DataTable, Column, Tag, Chip, Button, Dialog, Timeline, Divider, Card, } from 'primevue';
import NetworkChart from './NetworkChart.vue';
import VpnPortalDialog from './VpnPortalDialog.vue';

const props = defineProps<{
  curNetworkInst: NetworkInstance | null,
  api: RemoteClient,
  readonly?: boolean,
  managed?: boolean,
  desktopView?: 'overview' | 'connections' | 'devices',
  networkName?: string,
  requestedPeerId?: string | null,
}>()

const emit = defineEmits<{ copyAddress: [address: string]; devicePeers: [peers: DesktopPanelPeer[]] }>()
const { t, locale } = useI18n()
const networkConfig = ref<NetworkConfig>()
const managedDesktop = computed(() => Boolean(props.desktopView && props.managed))

const peerRouteInfos = computed<PeerRoutePair[]>(() => {
  if (props.curNetworkInst) {
    const my_node_info = props.curNetworkInst.detail?.my_node_info
    return [{
      route: {
        peer_id: my_node_info?.peer_id ?? 0,
        ipv4_addr: my_node_info?.virtual_ipv4 ?? null,
        ipv6_addr: my_node_info?.virtual_ipv6,
        hostname: my_node_info?.hostname ?? '',
        version: my_node_info?.version ?? '',
        stun_info: my_node_info?.stun_info,
        cost: 0,
        next_hop_peer_id: 0,
        proxy_cidrs: [],
        inst_id: props.curNetworkInst.instance_id,
      },
    }, ...(props.curNetworkInst.detail?.peer_route_pairs || [])]
  }

  return []
})

function routeCost(info: PeerRoutePair) {
  if (!info.route.cost) return t('status.local')
  if (info.route.cost > 1) return t(props.desktopView ? 'connection_path.fallback_relay' : 'connection_path.relay')
  if (isForwardingNode(info)) return t('connection_path.forwarding_node')
  if (props.desktopView) return t('connection_path.p2p_direct')
  return t(props.managed ? 'connection_path.device_direct' : 'connection_path.direct')
}

function resolveObjPath(path: string, obj: any = globalThis, separator = '.') {
  const properties = path.split(separator)
  return properties.reduce((prev, curr) => prev?.[curr], obj)
}

function statsCommon(info: any, field: string): number | undefined {
  if (!info.peer)
    return undefined

  let sum = 0
  let hasValue = false
  for (const conn of peerConns(info)) {
    const value = numericValue(resolveObjPath(field, conn))
    if (value === undefined)
      continue

    sum += value
    hasValue = true
  }
  return hasValue ? sum : undefined
}

function humanFileSize(bytes: number, si = false, dp = 1) {
  const thresh = si ? 1000 : 1024

  if (Math.abs(bytes) < thresh)
    return `${bytes} B`

  const units = si
    ? ['kB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB']
    : ['KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB', 'ZiB', 'YiB']
  let u = -1
  const r = 10 ** dp

  do {
    bytes /= thresh
    ++u
  } while (Math.round(Math.abs(bytes) * r) / r >= thresh && u < units.length - 1)

  return `${bytes.toFixed(dp)} ${units[u]}`
}

function txBytes(info: PeerRoutePair) {
  const tx = statsCommon(info, 'stats.tx_bytes')
  return tx ? humanFileSize(tx) : ''
}

function rxBytes(info: PeerRoutePair) {
  const rx = statsCommon(info, 'stats.rx_bytes')
  return rx ? humanFileSize(rx) : ''
}

function ipFormat(info: PeerRoutePair) {
  const ip = info.route.ipv4_addr
  if (typeof ip === 'string')
    return ip
  return ip ? ipv4InetToString(ip) : ''
}

function oneTunnelProto(tunnel?: TunnelInfo): string {
  if (!tunnel || isRetiredTransport(tunnel.tunnel_type))
    return ''

  const local_addr = tunnel.local_addr
  let isIPv6 = false;
  if (local_addr?.url) {
    try {
      const urlObj = new URL(local_addr.url, 'http://dummy');
      // IPv6 addresses in URLs are enclosed in brackets and contain ':'
      isIPv6 = /^\[.*:.*\]$/.test(urlObj.hostname);
    } catch (e) {
      // fallback to original check if URL parsing fails
      isIPv6 = local_addr.url.indexOf('[') >= 0;
    }
  }
  if (isIPv6)
    return `${tunnel.tunnel_type}6`
  else
    return tunnel.tunnel_type
}

function tunnelProto(info: PeerRoutePair) {
  return [...new Set(peerConns(info).map(c => oneTunnelProto(c.tunnel)).filter(Boolean))].join(',')
}

const peersById = computed(() => {
  const peers = new Map(props.curNetworkInst?.detail?.peers?.map(peer => [peer.peer_id, peer]))
  for (const info of peerRouteInfos.value) {
    if (info.peer) peers.set(info.route.peer_id, info.peer)
  }
  return peers
})

function desktopPathPeer(info: PeerRoutePair) {
  return info.route.cost > 1 ? peersById.value.get(info.route.next_hop_peer_id) : info.peer
}

function desktopPathConnection(peer: PeerRoutePair['peer']) {
  return preferredConn({ peer }) ?? (peer?.conns.length === 1 ? peer.conns[0] : undefined)
}

function desktopTunnelProto(info: PeerRoutePair) {
  const peer = desktopPathPeer(info)
  const active = desktopPathConnection(peer)
  if (active)
    return oneTunnelProto(active.tunnel)

  const protocols = new Set(peer?.conns.map(conn => oneTunnelProto(conn.tunnel)).filter(Boolean))
  return protocols.size === 1 ? [...protocols][0] : ''
}

function desktopPathQuality(info: PeerRoutePair) {
  const active = desktopPathConnection(desktopPathPeer(info))
  const latency = numericValue(active?.stats?.latency_us)
  const loss = numericValue(active?.loss_rate)
  const measured = latency !== undefined && latency > 0
  return {
    latency: measured ? `${Math.ceil(latency / 1000)}ms` : '—',
    loss: loss !== undefined || measured ? `${Math.round((loss ?? 0) * 100)}%` : '—',
  }
}

function endpoint(url?: string) {
  if (!url) return ''
  try {
    return new URL(url).host.toLowerCase()
  } catch {
    return ''
  }
}

function connectedNodeAddress(info: PeerRoutePair) {
  const preferred = preferredConn(info)?.tunnel
  const address = (tunnel?: TunnelInfo) => endpoint(tunnel?.resolved_remote_addr?.url) || endpoint(tunnel?.remote_addr?.url)
  if (preferred) return address(preferred)

  const addresses = new Set(peerConns(info).map(conn => address(conn.tunnel)).filter(Boolean))
  return addresses.size === 1 ? [...addresses][0] : ''
}

function desktopConnectionName(info: PeerRoutePair) {
  if (managedDesktop.value && isForwardingNode(info)) {
    return connectedNodeAddress(info) || info.route.hostname
  }
  return info.route.hostname
}

const routesById = computed(() => new Map(peerRouteInfos.value.map(info => [info.route.peer_id, info])))

function desktopRelayAddress(info: PeerRoutePair) {
  if (!managedDesktop.value || info.route.cost <= 1) return ''

  const hop = routesById.value.get(info.route.next_hop_peer_id)
  return hop && isForwardingNode(hop) ? desktopConnectionName(hop) : ''
}

const myNodeInfo = computed(() => {
  if (!props.curNetworkInst)
    return {} as NodeInfo

  return props.curNetworkInst.detail?.my_node_info
})

interface Chip {
  label: string
  icon: string
}

const myNodeInfoChips = computed(() => {
  if (!props.curNetworkInst)
    return []

  const chips: Array<Chip> = []
  const my_node_info = props.curNetworkInst.detail?.my_node_info
  if (!my_node_info)
    return chips

  // peer id
  chips.push({
    label: `${t('desktop.peer_id')} · ${my_node_info.peer_id}`,
    icon: '',
  } as Chip)

  // TUN Device Name
  const dev_name = props.curNetworkInst.detail?.dev_name
  if (dev_name) {
    chips.push({
      label: `${t('desktop.interface')} · ${dev_name}`,
      icon: '',
    } as Chip)
  }

  // virtual ipv4
  chips.push({
    label: `${t('virtual_ipv4')} · ${desktopAddress.value}`,
    icon: '',
  } as Chip)

  // local ipv4s
  const local_ipv4s = my_node_info.ips?.interface_ipv4s
  for (const ip of local_ipv4s ?? []) {
    chips.push({
      label: `${t('desktop.local_ipv4')} · ${ipv4ToString(ip)}`,
      icon: '',
    } as Chip)
  }

  // local ipv6s
  const local_ipv6s = my_node_info.ips?.interface_ipv6s
  for (const ip of local_ipv6s ?? []) {
    chips.push({
      label: `${t('desktop.local_ipv6')} · ${ipv6ToString(ip)}`,
      icon: '',
    } as Chip)
  }

  // public ip
  const public_ip = my_node_info.ips?.public_ipv4
  if (public_ip) {
    chips.push({
      label: `${t('desktop.public_ipv4')} · ${ipv4ToString(public_ip)}`,
      icon: '',
    } as Chip)
  }

  const public_ipv6 = my_node_info.ips?.public_ipv6
  if (public_ipv6) {
    chips.push({
      label: `${t('desktop.public_ipv6')} · ${ipv6ToString(public_ipv6)}`,
      icon: '',
    } as Chip)
  }

  // listeners:
  const listeners = my_node_info.listeners
  for (const listener of listeners ?? []) {
    chips.push({
      label: `${t('desktop.listener')} · ${listener.url}`,
      icon: '',
    } as Chip)
  }

  const udpNatType = renderNatType(my_node_info.stun_info)
  if (udpNatType) {
    chips.push({
      label: `${t('nat_type')} · ${udpNatType}`,
      icon: '',
    } as Chip)
  }

  return chips
})

function globalSumCommon(field: string) {
  let sum = 0
  if (!peerRouteInfos.value)
    return sum

  for (const info of peerRouteInfos.value) {
    const tx = statsCommon(info, field)
    if (tx)
      sum += tx
  }
  return sum
}

function txGlobalSum() {
  return globalSumCommon('stats.tx_bytes')
}

function rxGlobalSum() {
  return globalSumCommon('stats.rx_bytes')
}

function natType(info: PeerRoutePair): string {
  return renderNatType(info.route?.stun_info)
}

function renderNatType(stun: Parameters<typeof udpNatTypeName>[0]): string {
  if (props.desktopView) return udpNatLevel(stun) ?? '—'
  const type = udpNatTypeName(stun)
  if (!type || type === 'Unknown') {
    return t(stun?.public_ip?.length ? 'nat_probe.mapped' : 'nat_probe.unavailable')
  }
  return t('nat_probe.' + type.toLowerCase().replace(/ /g, '_'))
}

function isPublicServerRoute(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.is_public_server ?? false
}

function hasVirtualAddress(info: PeerRoutePair): boolean {
  return Boolean(info.route.ipv4_addr || info.route.ipv6_addr)
}

function isForwardingNode(info: PeerRoutePair): boolean {
  if (isPublicServerRoute(info)) return true
  // The managed controller gives every member a virtual address. Its forwarding node has none.
  return Boolean(props.managed && info.route.cost === 1 && peerConns(info).length && !hasVirtualAddress(info))
}

function shouldAvoidRelayData(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.avoid_relay_data ?? false
}

const peerCount = computed(() => {
  if (!peerRouteInfos.value)
    return 0

  return peerRouteInfos.value.length
})

const desktopAddress = computed(() => {
  const address = myNodeInfo.value?.virtual_ipv4
  if (typeof address === 'string') return address
  return address ? ipv4InetToString(address) : '—'
})

const connectionQuery = ref('')
const desktopConnections = computed(() => {
  const routes = peerRouteInfos.value
  if (!managedDesktop.value) return routes

  const isRelayInfrastructure = (info: PeerRoutePair) =>
    info.route.cost > 0 && (isPublicServerRoute(info) || !hasVirtualAddress(info))
  const connectedNodes = routes.filter(info =>
    isRelayInfrastructure(info) && info.route.cost === 1 && peerConns(info).length > 0)
  const assignedEndpoints = new Set((networkConfig.value?.peer_urls ?? []).map(endpoint).filter(Boolean))
  const activeNode = connectedNodes.length === 1 ? connectedNodes[0] : connectedNodes.find(info =>
    peerConns(info).some(({ tunnel }) => assignedEndpoints.has(endpoint(tunnel?.remote_addr?.url))
      || assignedEndpoints.has(endpoint(tunnel?.resolved_remote_addr?.url))))
  return routes.filter(info => info === activeNode || !isRelayInfrastructure(info))
})

const filteredConnections = computed(() => {
  const query = connectionQuery.value.trim().toLocaleLowerCase()
  return desktopConnections.value.filter(info => !query
    || [info.route.hostname, desktopConnectionName(info), ipFormat(info), routeCost(info), desktopRelayAddress(info)]
      .some(value => value?.toLocaleLowerCase().includes(query)))
})

const deviceQuery = ref('')
const selectedDeviceKey = ref<string>()

function deviceKey(device: PeerRoutePair) {
  return device.route.cost ? String(device.route.peer_id) : 'local'
}

function deviceName(device: PeerRoutePair) {
  return desktopConnectionName(device) || (device.route.cost ? '—' : t('status.local'))
}

function deviceAddress(device: PeerRoutePair) {
  const address = device.route.ipv4_addr
  return (typeof address === 'string' ? address : address?.address && ipv4InetToString(address))?.split('/')[0] ?? ''
}

const workspaceDevices = computed(() => desktopConnections.value.filter(device => !isForwardingNode(device)))
const filteredDevices = computed(() => {
  const query = deviceQuery.value.trim().toLocaleLowerCase()
  return workspaceDevices.value.filter(device => [deviceName(device), deviceAddress(device), ipv6ToString(device.route.ipv6_addr?.address)]
    .some(value => value.toLocaleLowerCase().includes(query)))
})
const selectedDevice = computed(() => filteredDevices.value.find(device => deviceKey(device) === selectedDeviceKey.value)
  ?? filteredDevices.value.find(device => device.route.cost > 0)
  ?? filteredDevices.value[0])
const selectedDeviceIPv6 = computed(() => ipv6ToString(selectedDevice.value?.route.ipv6_addr?.address))
const selectedDeviceAddress = computed(() => selectedDevice.value ? deviceAddress(selectedDevice.value) || selectedDeviceIPv6.value : '')

watch(() => props.curNetworkInst?.instance_id, () => {
  selectedDeviceKey.value = props.requestedPeerId || undefined
  deviceQuery.value = ''
})

watch(() => props.requestedPeerId, (id) => {
  if (!id) return
  selectedDeviceKey.value = id
  deviceQuery.value = ''
}, { immediate: true })

watch(workspaceDevices, (devices) => {
  if (!props.desktopView) return
  emit('devicePeers', devices.filter(device => device.route.cost > 0).map(device => ({
    id: deviceKey(device),
    name: deviceName(device),
    address: deviceAddress(device) || ipv6ToString(device.route.ipv6_addr?.address),
    online: true,
    icon: 'pi pi-desktop',
  })))
}, { immediate: true })

// calculate tx/rx rate every 2 seconds
let rateIntervalId = 0
const rateInterval = 2000
let prevTxSum: number | undefined
let prevRxSum: number | undefined
const txRate = ref('0')
const rxRate = ref('0')

// 控制节点详细信息chips的显示/隐藏
const showNodeDetails = ref(false)

onMounted(() => {
  rateIntervalId = window.setInterval(() => {
    const curTxSum = txGlobalSum()
    txRate.value = humanFileSize(Math.max(0, curTxSum - (prevTxSum ?? curTxSum)) / (rateInterval / 1000))
    prevTxSum = curTxSum

    const curRxSum = rxGlobalSum()
    rxRate.value = humanFileSize(Math.max(0, curRxSum - (prevRxSum ?? curRxSum)) / (rateInterval / 1000))
    prevRxSum = curRxSum
  }, rateInterval)
})

onUnmounted(() => {
  clearInterval(rateIntervalId)
})

const dialogVisible = ref(false)
const dialogContent = ref<{ time: string; event: unknown }[]>([])
const vpnPortalVisible = ref(false)
const vpnPortalEnabled = computed(() => props.curNetworkInst?.running
  && networkConfig.value?.vpn_portal_config !== undefined
  && networkConfig.value.vpn_portal_config.enabled !== false)

watch(() => props.curNetworkInst?.instance_id, async (instanceId, _, onCleanup) => {
  let active = true
  vpnPortalVisible.value = false
  networkConfig.value = undefined
  if (!instanceId) return
  let refreshTimer: ReturnType<typeof setTimeout> | undefined
  onCleanup(() => {
    active = false
    clearTimeout(refreshTimer)
  })
  const refresh = async () => {
    try {
      const config = await props.api.get_network_config(instanceId)
      if (active) networkConfig.value = config
    } catch (error) {
      console.error('Failed to load WireGuard settings', error)
    } finally {
      if (active) refreshTimer = setTimeout(refresh, 10_000)
    }
  }
  await refresh()
}, { immediate: true })

function showEventLogs() {
  const detail = props.curNetworkInst?.detail
  if (!detail)
    return

  dialogContent.value = (detail.events ?? []).flatMap(event => {
    try { return [JSON.parse(event)] } catch { return [] }
  })
  dialogVisible.value = true
}

function eventTime(value: string) {
  return new Date(value).toLocaleString(locale.value === 'cn' ? 'zh-CN' : 'en-US')
}
</script>

<template>
  <div class="frontend-lib">
    <Dialog v-model:visible="dialogVisible" modal :header="t('event_log')"
      :class="desktopView ? 'desk-log-dialog' : 'w-full h-auto max-h-full'"
      :baseZIndex="2000">
      <p v-if="dialogContent.length === 0" class="desk-log-empty">{{ t('desktop.no_events') }}</p>
      <Timeline v-else :value="dialogContent">
        <template #opposite="slotProps">
          <small class="text-surface-500 dark:text-surface-400">{{ desktopView
            ? eventTime(slotProps.item.time) : useTimeAgo(Date.parse(slotProps.item.time)) }}</small>
        </template>
        <template #content="slotProps">
          <HumanEvent :event="slotProps.item.event" />
        </template>
      </Timeline>
    </Dialog>

    <VpnPortalDialog v-if="vpnPortalVisible && curNetworkInst && vpnPortalEnabled" :key="curNetworkInst.instance_id"
      :instance="curNetworkInst" :api="api" :readonly="readonly" @close="vpnPortalVisible = false" />

    <Card v-if="curNetworkInst?.error_msg">
      <template #title>
        Run Network Error
      </template>
      <template #content>
        <div class="flex flex-col gap-y-5">
          <div class="text-red-500">
            {{ curNetworkInst.error_msg }}
          </div>
        </div>
      </template>
    </Card>

    <template v-else-if="desktopView">
      <section v-if="desktopView === 'devices'" class="desk-device-workspace" :aria-label="t('desktop.network_devices')">
        <aside class="desk-device-list-panel">
          <label class="desk-search desk-device-search">
            <i class="pi pi-search" aria-hidden="true" />
            <input v-model="deviceQuery" type="search" :aria-label="t('desktop.search_devices')"
              :placeholder="t('desktop.search_devices')" />
          </label>
          <h2>{{ t('desktop.network_devices') }}</h2>
          <ul class="desk-device-list">
            <li v-for="device in filteredDevices" :key="deviceKey(device)">
              <button type="button" class="desk-device-row" :aria-pressed="device === selectedDevice"
                @click="selectedDeviceKey = deviceKey(device)">
                <i class="pi pi-desktop desk-device-icon" aria-hidden="true" />
                <span class="desk-device-label">
                  <span class="desk-device-name" :title="deviceName(device)">
                    {{ deviceName(device) }}
                    <small v-if="!device.route.cost">{{ t('status.local') }}</small>
                  </span>
                  <span class="desk-device-secondary">
                    <i class="desk-device-online" aria-hidden="true" />
                    <code>{{ deviceAddress(device) || ipv6ToString(device.route.ipv6_addr?.address) || '—' }}</code>
                  </span>
                </span>
                <i v-if="device === selectedDevice" class="pi pi-angle-right desk-device-chevron" aria-hidden="true" />
              </button>
            </li>
          </ul>
          <p v-if="!filteredDevices.length" class="desk-device-placeholder">{{ t('desktop.no_connections_found') }}</p>
        </aside>
        <div class="desk-device-detail">
          <template v-if="selectedDevice">
            <header class="desk-device-detail-heading">
              <i class="pi pi-desktop desk-device-icon" aria-hidden="true" />
              <div>
                <h2>{{ deviceName(selectedDevice) }}</h2>
                <span class="desk-device-secondary"><i class="desk-device-online" aria-hidden="true" />{{ t('controller.online') }}</span>
              </div>
            </header>
            <dl class="desk-device-detail-facts">
              <div><dt>{{ t('virtual_ipv4') }}</dt><dd><code>{{ deviceAddress(selectedDevice) || '—' }}</code></dd></div>
              <div v-if="selectedDeviceIPv6"><dt>{{ t('desktop.virtual_ipv6') }}</dt><dd><code>{{ selectedDeviceIPv6 }}</code></dd></div>
              <div><dt>{{ t('connection_path.title') }}</dt><dd>{{ routeCost(selectedDevice) }}</dd></div>
              <div><dt>{{ t('desktop.transport_protocol') }}</dt><dd>{{ desktopTunnelProto(selectedDevice).toUpperCase() || '—' }}</dd></div>
              <div><dt>{{ t('latency') }}</dt><dd><code>{{ desktopPathQuality(selectedDevice).latency }}</code></dd></div>
            </dl>
            <Button v-if="selectedDeviceAddress" class="desk-device-copy" size="small" icon="pi pi-copy"
              :label="t('desktop.copy_address')" @click="emit('copyAddress', selectedDeviceAddress)" />
          </template>
          <p v-else class="desk-device-placeholder">{{ t('desktop.no_connections_found') }}</p>
        </div>
      </section>

      <div v-show="desktopView === 'overview'" class="desk-dashboard">
        <div class="desk-summary">
          <section class="desk-hero" :aria-label="t('desktop.network')">
            <div class="desk-hero-top">
              <span><i class="pi pi-sitemap" aria-hidden="true" /> {{ t('desktop.current_network') }}</span>
              <span class="desk-running"><i aria-hidden="true" />{{ t(curNetworkInst?.running ? 'network_running' : 'network_stopped') }}</span>
            </div>
            <h2>{{ networkName || t('controller.assigned_network') }}</h2>
            <dl class="desk-metrics">
              <div><dt>{{ t('virtual_ipv4') }}</dt><dd>{{ desktopAddress }}</dd></div>
              <div><dt>{{ t('desktop.remote_connections') }}</dt><dd>{{ Math.max(0, peerCount - 1) }}</dd></div>
            </dl>
          </section>

          <section class="desk-panel desk-local">
            <header><h2><i class="pi pi-desktop" aria-hidden="true" /> {{ t('desktop.local_device') }}</h2></header>
            <dl class="desk-facts">
              <div><dt>{{ t('hostname') }}</dt><dd>{{ myNodeInfo?.hostname || '—' }}</dd></div>
              <div><dt>{{ t('nat_type') }}</dt><dd>{{ renderNatType(myNodeInfo?.stun_info) }}</dd></div>
              <div><dt>{{ t('status.version') }}</dt><dd :title="myNodeInfo?.version">{{ myNodeInfo?.version || '—' }}</dd></div>
            </dl>
          </section>
        </div>

        <section class="desk-panel desk-traffic">
          <header>
            <h2><i class="pi pi-chart-line" aria-hidden="true" /> {{ t('desktop.traffic') }}</h2>
            <button v-if="vpnPortalEnabled" type="button" class="desk-quiet-button" @click="vpnPortalVisible = true">
              {{ t('vpn_portal_devices') }} <i class="pi pi-arrow-right" aria-hidden="true" />
            </button>
          </header>
          <NetworkChart :key="curNetworkInst?.instance_id" :upload-rate="txRate" :download-rate="rxRate" monochrome />
        </section>
      </div>

      <section v-show="desktopView === 'connections'" class="desk-panel desk-connections">
        <header>
          <div class="desk-search">
            <i class="pi pi-search" aria-hidden="true" />
            <input v-model="connectionQuery" type="search" :aria-label="t('desktop.search_connections')"
              :placeholder="t('desktop.search_placeholder')" />
          </div>
          <button type="button" class="desk-quiet-button" @click="showEventLogs">
            <i class="pi pi-list" aria-hidden="true" /> {{ t('event_log') }}
          </button>
        </header>
        <div class="desk-table-scroll" tabindex="0" role="region" :aria-label="t('connection_path.devices_and_nodes')">
          <table class="desk-table">
            <thead>
              <tr><th scope="col">{{ t('hostname') }}</th><th scope="col">{{ t('virtual_ipv4') }}</th>
                <th scope="col">{{ t('connection_path.title') }}</th><th scope="col">{{ t('latency') }}</th>
                <th scope="col">{{ t('desktop.probe_loss') }}</th><th scope="col">{{ t('nat_type') }}</th>
                <th scope="col">{{ t('upload_bytes') }} / {{ t('download_bytes') }}</th>
                <th scope="col">{{ t('status.version') }}</th></tr>
            </thead>
            <tbody>
              <tr v-for="info in filteredConnections" :key="info.route.cost ? info.route.peer_id : 'local'">
                <th scope="row"><span class="desk-device-cell">
                  <i :class="isForwardingNode(info) ? 'pi pi-server' : 'pi pi-desktop'" aria-hidden="true" />
                  <span :title="desktopConnectionName(info)">{{ desktopConnectionName(info) || '—' }}</span>
                </span></th>
                <td class="desk-data">{{ ipFormat(info) || '—' }}</td>
                <td>
                  <span class="desk-path">{{ routeCost(info) }}</span>
                  <small v-if="desktopTunnelProto(info)">{{ desktopTunnelProto(info).toUpperCase() }}</small>
                  <small v-if="desktopRelayAddress(info)">{{ desktopRelayAddress(info) }}</small>
                </td>
                <td class="desk-data">{{ desktopPathQuality(info).latency }}</td>
                <td class="desk-data">{{ desktopPathQuality(info).loss }}</td>
                <td>{{ natType(info) }}</td><td class="desk-data"><span>{{ txBytes(info) || '—' }}</span><small>{{ rxBytes(info) || '—' }}</small></td>
                <td class="desk-data" :title="info.route.version">{{ info.route.version || '—' }}</td>
              </tr>
              <tr v-if="!filteredConnections.length"><td colspan="8" class="desk-no-results">{{ t('desktop.no_connections_found') }}</td></tr>
            </tbody>
          </table>
        </div>
      </section>
    </template>

    <template v-else>
      <Card>
        <template #title>
          {{ t('my_node_info') }}
        </template>
        <template #content>
          <div class="flex w-full flex-col gap-y-5">
            <div class="gap-4">
              <!-- 网络流量图表 -->
              <div class="w-full">
                <NetworkChart :upload-rate="txRate" :download-rate="rxRate" />
              </div>
            </div>

            <!-- 展开/收起节点详细信息的divider按钮 -->
            <div class="w-full">
              <Button @click="showNodeDetails = !showNodeDetails"
                :icon="showNodeDetails ? 'pi pi-chevron-up' : 'pi pi-chevron-down'"
                :label="showNodeDetails ? t('hide_node_details') : t('show_node_details')" severity="secondary" outlined
                class="w-full justify-center" size="small" />
            </div>

            <!-- 节点详细信息chips，根据showNodeDetails状态显示/隐藏 -->
            <div v-show="showNodeDetails" class="flex flex-row items-center flex-wrap w-full max-h-40 overflow-scroll">
              <Chip v-for="(chip, i) in myNodeInfoChips" :key="i" :label="chip.label" :icon="chip.icon"
                class="mr-2 mt-2 text-sm" />
            </div>

            <div v-if="myNodeInfo" class="m-0 flex flex-row justify-center gap-x-5 text-sm">
              <Button v-if="vpnPortalEnabled" severity="info" :label="t('vpn_portal_devices')" @click="vpnPortalVisible = true" />
              <Button severity="info" :label="t('show_event_log')" @click="showEventLogs" />
            </div>
          </div>
        </template>
      </Card>

      <Divider />

      <Card>
        <template #title>
          <div class="flex items-center gap-3">
            <div class="flex items-center gap-2">
              <span>{{ t(props.managed ? 'connection_path.devices_and_nodes' : 'peer_info') }}</span>
            </div>
            <div class="flex items-center gap-1">
              <Badge :value="peerCount" severity="info"
                class="text-lg font-semibold px-2 py-1 rounded-full bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200" />
            </div>
          </div>
        </template>
        <template #content>
          <DataTable :value="peerRouteInfos" column-resize-mode="fit" table-class="w-full">
            <Column :field="ipFormat" :header="t('virtual_ipv4')" />
            <Column :header="t('hostname')">
              <template #body="slotProps">
                <div v-if="!slotProps.data.route.cost || !isPublicServerRoute(slotProps.data) || props.managed"
                  v-tooltip="slotProps.data.route.hostname">
                  {{
                    slotProps.data.route.hostname }}
                </div>
                <div v-else v-tooltip="slotProps.data.route.hostname" class="space-x-1">
                  <Tag v-if="isPublicServerRoute(slotProps.data)" severity="info" value="Info">
                    {{ t('status.server') }}
                  </Tag>
                  <Tag v-if="shouldAvoidRelayData(slotProps.data)" severity="warn" value="Warn">
                    {{ t('status.relay') }}
                  </Tag>
                </div>
              </template>
            </Column>
            <Column :field="routeCost" :header="t('connection_path.title')" />
            <Column :field="tunnelProto" :header="t('tunnel_proto')" />
            <Column :field="latencyMs" :header="t('latency')" />
            <Column :field="txBytes" :header="t('upload_bytes')" />
            <Column :field="rxBytes" :header="t('download_bytes')" />
            <Column :field="lossRate" :header="t('loss_rate')" />
            <Column :field="natType" :header="t('nat_type')" />
            <Column :header="t('status.version')">
              <template #body="slotProps">
                <span>{{ slotProps.data.route.version || '—' }}</span>
              </template>
            </Column>
          </DataTable>
        </template>
      </Card>
    </template>
  </div>
</template>

<style lang="postcss" scoped>
.p-timeline :deep(.p-timeline-event-opposite) {
  @apply flex-none;
}

:deep(.p-datatable .p-datatable-column-title) {
  white-space: nowrap;
}

.desk-dashboard { display: grid; gap: 18px; }
.desk-summary { display: grid; grid-template-columns: minmax(0, 1.45fr) minmax(235px, 1fr); gap: 18px; }
.desk-hero, .desk-panel { border-radius: 12px; min-width: 0; }
.desk-hero {
  position: relative;
  isolation: isolate;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  padding: 22px 24px;
  min-height: 218px;
  background: #202020;
  color: #fff;
}

.desk-hero::after {
  content: '';
  position: absolute;
  z-index: -1;
  inset: 0 0 0 25%;
  background: radial-gradient(circle, #5a5a5a 1px, transparent 1.3px) 100% 0 / 13px 13px;
  mask-image: linear-gradient(90deg, transparent, #000 88%);
  opacity: .6;
}

.desk-hero-top { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 10px; }
.desk-hero-top > span:first-child { color: #d4d4d4; font-size: 12px; }
.desk-hero-top > span:first-child i { margin-right: 5px; }
.desk-running {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  border: 1px solid #626262;
  background: #323232;
  border-radius: 24px;
  padding: 4px 10px;
  font-size: 12px;
}
.desk-running i { width: 6px; height: 6px; background: #ededed; border-radius: 100%; }
.desk-hero h2 { margin: 16px 0 20px; overflow-wrap: anywhere; font-size: clamp(22px, 4vw, 36px); line-height: 1.15; font-weight: 660; }
.desk-metrics { display: flex; flex-wrap: wrap; gap: 12px 27px; margin: auto 0 0; }
.desk-metrics > div + div { padding-left: 25px; border-left: 1px solid #505050; }
.desk-metrics dt { color: #c0c0c0; font-size: 12px; }
.desk-metrics dd { margin: 3px 0 0; font-size: 18px; font-family: var(--desk-mono, monospace); font-weight: 580; }
.desk-panel { background: #fff; border: 1px solid #e1e1e1; color: #191919; }
.desk-local, .desk-traffic { padding: 20px 22px; }
.desk-panel header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.desk-panel h2 { margin: 0; font-weight: 650; font-size: 15px; display: flex; align-items: center; gap: 8px; }
.desk-panel h2 .pi { color: #626262; font-size: 15px; }
.desk-facts { margin: 12px 0 0; }
.desk-facts > div { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding: 7px 0; border-bottom: 1px solid #ededed; font-size: 12px; }
.desk-facts > div:last-child { border-bottom: 0; }
.desk-facts dt { color: #626262; flex: 0 0 auto; }
.desk-facts dd { margin: 0; text-align: right; overflow-wrap: anywhere; font-weight: 560; }
.desk-quiet-button {
  padding: 7px 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  color: #292929;
  font-size: 12px;
  font-weight: 600;
  text-align: left;
}
.desk-quiet-button:hover { color: #000; text-decoration: underline; }
.desk-quiet-button .pi { font-size: 11px; }
.desk-traffic header { margin-bottom: 14px; }
.desk-connections header { padding: 14px 18px; }
.desk-connections .desk-quiet-button { border: 1px solid #ddd; padding: 7px 12px; border-radius: 7px; white-space: nowrap; }
.desk-search { display: flex; align-items: center; gap: 8px; flex: 0 1 242px; min-width: 0; color: #656565; padding: 0 10px; border: 1px solid #dedede; border-radius: 7px; }
.desk-search input { padding: 7px 0; width: 100%; min-width: 0; color: #151515; background: transparent; font-size: 12px; }
.desk-search:focus-within { outline: 2px solid #444; outline-offset: 1px; }
.desk-search input:focus { outline: 0; }
.desk-table-scroll { overflow-x: auto; border-radius: 0 0 12px 12px; }
.desk-table { width: 100%; white-space: nowrap; border-collapse: collapse; font-size: 12px; }
.desk-table th, .desk-table td { text-align: left; padding: 13px 12px; border-top: 1px solid #e9e9e9; }
.desk-table th:first-child { padding-left: 18px; }
.desk-table th:last-child, .desk-table td:last-child { padding-right: 18px; }
.desk-table thead th { color: #5e5e5e; background: #f7f7f7; font-size: 11px; font-weight: 650; }
.desk-table th[scope=row] { font-weight: 600; }
.desk-table tbody tr:hover { background: #fafafa; }
.desk-device-cell { display: flex; align-items: center; gap: 9px; }
.desk-device-cell > i { display: grid; place-items: center; width: 30px; height: 30px; border-radius: 7px; background: #eee; color: #444; }
.desk-device-cell > span { max-width: 19ch; overflow: hidden; text-overflow: ellipsis; }
.desk-table small { display: block; color: #626262; font-size: 10px; }
.desk-path { font-weight: 580; }
.desk-data { font-family: var(--desk-mono, monospace); font-variant-numeric: tabular-nums; }
.desk-table .desk-no-results { text-align: center; color: #626262; padding: 37px 12px; }
.desk-log-dialog { width: min(660px, calc(100vw - 28px)); max-height: min(600px, 85dvh); }
.desk-log-empty { color: #626262; padding: 25px 0; text-align: center; }

.desk-device-workspace {
  display: grid;
  grid-template-columns: clamp(212px, 29%, 360px) minmax(0, 1fr);
  min-height: 310px;
  overflow: hidden;
  border: 1px solid #e0e0e0;
  border-radius: 10px;
  background: #fff;
  color: #191919;
}

.desk-device-list-panel { min-width: 0; padding: 12px; border-right: 1px solid #e5e5e5; background: #f7f7f7; }
.desk-device-search { min-height: 34px; background: #fff; }
.desk-device-search input::placeholder { color: #6a6a6a; opacity: 1; }
.desk-device-list-panel h2 { margin: 12px 3px 8px; color: #454545; font-size: 12px; font-weight: 700; }
.desk-device-list { margin: 0; padding: 0; list-style: none; }
.desk-device-row {
  display: flex;
  width: 100%;
  min-height: 54px;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  border: 1px solid transparent;
  border-radius: 8px;
  text-align: left;
}
.desk-device-list li + li .desk-device-row { border-top-color: #e7e7e7; }
.desk-device-row:hover { background: #eeeeee; }
.desk-device-row:focus-visible { outline-offset: -2px; }
.desk-device-row[aria-pressed='true'] { border-color: #dadada; background: #ececec; }
.desk-device-icon {
  display: grid;
  place-items: center;
  flex: 0 0 30px;
  height: 30px;
  border: 1px solid #dedede;
  border-radius: 8px;
  background: #f1f1f1;
  color: #454545;
  font-size: 14px;
}
.desk-device-label { flex: 1 1 auto; min-width: 0; }
.desk-device-name { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; line-height: 1.25; font-weight: 700; }
.desk-device-name small { margin-left: 4px; color: #636363; font-size: 10px; font-weight: 500; }
.desk-device-secondary { display: flex; align-items: center; gap: 6px; margin-top: 4px; min-width: 0; color: #5c5c5c; font-size: 11px; }
.desk-device-secondary code { overflow: hidden; text-overflow: ellipsis; font: 11px var(--desk-mono, monospace); }
.desk-device-online { flex: 0 0 6px; height: 6px; border-radius: 50%; background: #24965a; }
.desk-device-chevron { color: #6a6a6a; font-size: 11px; }
.desk-device-detail { min-width: 0; padding: 19px 20px 16px; }
.desk-device-detail-heading { display: flex; align-items: center; gap: 10px; margin: 0 0 17px; }
.desk-device-detail-heading > div { min-width: 0; }
.desk-device-detail-heading > .desk-device-icon { flex-basis: 40px; height: 40px; border-radius: 10px; font-size: 15px; }
.desk-device-detail-heading h2 { margin: 0; font-size: clamp(17px, 3vw, 19px); line-height: 1.25; letter-spacing: -.025em; font-weight: 750; overflow-wrap: anywhere; }
.desk-device-detail-facts { margin: 0 0 16px; border-top: 1px solid #e6e6e6; }
.desk-device-detail-facts > div { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; padding: 9px 0; border-bottom: 1px solid #e6e6e6; font-size: 12px; }
.desk-device-detail-facts dt { color: #5c5c5c; }
.desk-device-detail-facts dd { margin: 0; text-align: right; font-weight: 600; overflow-wrap: anywhere; }
.desk-device-detail-facts code { font: 12px var(--desk-mono, monospace); }
.desk-device-placeholder { margin: 13px 0; font-size: 12px; color: #5c5c5c; }

@media (max-width: 530px) {
  .desk-device-workspace { grid-template-columns: 1fr; }
  .desk-device-list-panel { border-right: 0; border-bottom: 1px solid #e5e5e5; }
  .desk-device-detail { padding: 15px; }
}

@media (max-width: 750px) {
  .desk-summary { grid-template-columns: 1fr; }
  .desk-hero { min-height: 193px; padding: 18px; }
  .desk-local, .desk-traffic { padding: 16px; }
}
</style>
