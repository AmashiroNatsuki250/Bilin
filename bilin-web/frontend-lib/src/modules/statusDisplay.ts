import { NatType, type PeerRoutePair, type StunInfo } from '../types/network'

const udpNatTypeStrMap: Record<NatType, string> = {
  [NatType.Unknown]: 'Unknown',
  [NatType.OpenInternet]: 'Open Internet',
  [NatType.NoPAT]: 'No PAT',
  [NatType.FullCone]: 'Full Cone',
  [NatType.Restricted]: 'Restricted',
  [NatType.PortRestricted]: 'Port Restricted',
  [NatType.Symmetric]: 'Symmetric',
  [NatType.SymUdpFirewall]: 'Symmetric UDP Firewall',
  [NatType.SymmetricEasyInc]: 'Symmetric Easy Inc',
  [NatType.SymmetricEasyDec]: 'Symmetric Easy Dec',
}

export function numericValue(value: unknown): number | undefined {
  if (typeof value === 'number')
    return Number.isFinite(value) ? value : undefined

  if (typeof value !== 'string' || value.trim() === '')
    return undefined

  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

export function peerConns(info: PeerRoutePair) {
  return info.peer?.conns || []
}

function defaultConnId(info: Pick<PeerRoutePair, 'peer'>) {
  const defaultConn = info.peer?.default_conn_id
  if (!defaultConn)
    return undefined

  const part1 = defaultConn.part1 ?? 0
  const part2 = defaultConn.part2 ?? 0
  const part3 = defaultConn.part3 ?? 0
  const part4 = defaultConn.part4 ?? 0
  if (part1 === 0 && part2 === 0 && part3 === 0 && part4 === 0)
    return undefined

  const toHex = (value: number) => value.toString(16).padStart(8, '0')
  const part1Hex = toHex(part1)
  const part2Hex = toHex(part2)
  const part3Hex = toHex(part3)
  const part4Hex = toHex(part4)
  return `${part1Hex}-${part2Hex.slice(0, 4)}-${part2Hex.slice(4, 8)}-${part3Hex.slice(0, 4)}-${part3Hex.slice(4, 8)}${part4Hex}`
}

export function preferredConn(info: Pick<PeerRoutePair, 'peer'>) {
  const id = defaultConnId(info)
  return id ? info.peer?.conns.find(conn => conn.conn_id === id) : undefined
}

function defaultConnFirst(info: PeerRoutePair) {
  const conns = peerConns(info)
  const defaultConn = preferredConn(info)
  return defaultConn ? [defaultConn, ...conns.filter(conn => conn !== defaultConn)] : conns
}

export function latencyMs(info: PeerRoutePair) {
  const connId = defaultConnId(info)
  let minLatencyUs: number | undefined

  for (const conn of peerConns(info)) {
    if (!conn.stats)
      continue

    const latencyUs = numericValue(conn.stats.latency_us)
    if (latencyUs === undefined)
      continue

    if (connId === conn.conn_id)
      return `${Math.ceil(latencyUs / 1000)}ms`

    minLatencyUs = Math.min(minLatencyUs ?? latencyUs, latencyUs)
  }

  if (minLatencyUs === undefined)
    return ''

  return `${Math.ceil(minLatencyUs / 1000)}ms`
}

export function lossRate(info: PeerRoutePair) {
  for (const conn of defaultConnFirst(info)) {
    const loss = numericValue(conn.loss_rate ?? 0)
    if (loss === undefined)
      continue

    return `${Math.round(loss * 100)}%`
  }

  return ''
}

function udpNatType(stunInfo: StunInfo | undefined): NatType {
  const value = stunInfo?.udp_nat_type ?? NatType.Unknown
  return numericValue(value) ?? NatType[value as keyof typeof NatType] ?? NatType.Unknown
}

export function udpNatTypeName(stunInfo: StunInfo | undefined) {
  if (!stunInfo) return ''
  return udpNatTypeStrMap[udpNatType(stunInfo)] ?? udpNatTypeStrMap[NatType.Unknown]
}

const udpNatLevels: Partial<Record<NatType, string>> = {
  [NatType.OpenInternet]: 'NAT1',
  [NatType.NoPAT]: 'NAT1',
  [NatType.FullCone]: 'NAT1',
  [NatType.Restricted]: 'NAT2',
  [NatType.PortRestricted]: 'NAT3',
  [NatType.Symmetric]: 'NAT4',
  [NatType.SymUdpFirewall]: 'NAT4',
  [NatType.SymmetricEasyInc]: 'NAT4',
  [NatType.SymmetricEasyDec]: 'NAT4',
}

export function udpNatLevel(stunInfo: StunInfo | undefined): string | undefined {
  return udpNatLevels[udpNatType(stunInfo)]
}
