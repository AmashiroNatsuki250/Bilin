import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import PrimeVue from 'primevue/config'
import { afterEach, describe, expect, it, vi } from 'vitest'
import Status from '../src/components/Status.vue'
import type { NetworkInstance } from '../src/types/network'

vi.mock('vue-i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }))

const wrappers: VueWrapper[] = []

afterEach(() => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount())
})

async function render(localStun: unknown, peerStun: unknown, lossRate?: number, managed = false, desktopView?: 'overview' | 'connections' | 'devices', assignedPeerUrls: string[] = []) {
  const instance = {
    instance_id: '12345678-9abc-def0-fedc-ba9876543210',
    running: true,
    detail: {
      my_node_info: {
        hostname: 'local-node',
        version: 'test',
        peer_id: 1,
        stun_info: localStun,
      },
      peer_route_pairs: [{
        route: {
          peer_id: 2,
          hostname: 'remote-node',
          version: 'test',
          cost: 1,
          stun_info: peerStun,
        },
        peer: {
          conns: [{
            conn_id: 'connection',
            ...(lossRate === undefined ? {} : { loss_rate: lossRate }),
          }],
        },
      }, ...(managed ? [
        { route: { peer_id: 3, hostname: 'direct-device', cost: 1, ipv4_addr: '10.250.0.2/24', stun_info: { udp_nat_type: 'Restricted' } } },
        { route: { peer_id: 4, hostname: 'relayed-device', cost: 2, ipv4_addr: '10.250.0.3/24', stun_info: { udp_nat_type: 9 } } },
      ] : [])],
    },
  } as unknown as NetworkInstance
  const wrapper = mount(Status, {
    props: {
      curNetworkInst: instance,
      api: { get_network_config: vi.fn(async () => ({ peer_urls: assignedPeerUrls })) } as any,
      managed,
      desktopView,
    },
    global: {
      plugins: [PrimeVue],
      directives: { tooltip: () => {} },
      stubs: { HumanEvent: true, NetworkChart: true, VpnPortalDialog: true },
    },
  })
  wrappers.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('status protobuf JSON rendering', () => {
  it('renders enum names and omitted zero loss in the real peer table', async () => {
    const wrapper = await render({ udp_nat_type: 'FullCone' }, { udp_nat_type: 'PortRestricted' })
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('nat_probe.full_cone')
    expect(rows[0].text()).not.toContain('0%')
    expect(rows[1].text()).toContain('nat_probe.port_restricted')
    expect(rows[1].text()).toContain('0%')
    await wrapper.findAll('button').find(button => button.text() === 'show_node_details')!.trigger('click')
    expect(wrapper.text()).toContain('nat_type · nat_probe.full_cone')
  })

  it('distinguishes a measured mapping without a classification from missing results', async () => {
    const wrapper = await render({ public_ip: ['192.0.2.2'] }, undefined, 0.25)
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('nat_probe.mapped')
    expect(rows[1].text()).toContain('nat_probe.unavailable')
    expect(rows[1].text()).toContain('25%')
    expect(wrapper.text()).toContain('nat_type · nat_probe.mapped')
  })

  it('preserves numeric NAT values from older backends', async () => {
    const wrapper = await render({ udp_nat_type: 1 }, { udp_nat_type: 6 }, 0.5)
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('nat_probe.open_internet')
    expect(rows[1].text()).toContain('nat_probe.symmetric')
    expect(rows[1].text()).toContain('50%')
  })

  it('labels the forwarding node separately from direct and relayed device traffic', async () => {
    const wrapper = await render(undefined, undefined, undefined, true)
    const rows = wrapper.findAll('tbody tr')
    expect(wrapper.text()).toContain('connection_path.devices_and_nodes')
    expect(rows[0].text()).toContain('status.local')
    expect(rows[1].text()).toContain('connection_path.forwarding_node')
    expect(rows[2].text()).toContain('connection_path.device_direct')
    expect(rows[3].text()).toContain('connection_path.relay')
  })

  it('shows four short NAT labels and actual connection paths on desktop', async () => {
    const wrapper = await render({ udp_nat_type: 5 }, { udp_nat_type: 'FullCone' }, 0, true, 'overview')
    expect(wrapper.get('.desk-local').text()).toContain('NAT3')
    expect(wrapper.findAll('button').some(button => button.text() === 'show_node_details')).toBe(false)
    await wrapper.setProps({ desktopView: 'connections' })

    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].get('td:nth-child(6)').text()).toBe('NAT3')
    expect(rows[1].get('td:nth-child(6)').text()).toBe('NAT1')
    expect(rows[1].text()).toContain('connection_path.forwarding_node')
    expect(rows[2].get('td:nth-child(6)').text()).toBe('NAT2')
    expect(rows[2].text()).toContain('connection_path.p2p_direct')
    expect(rows[3].get('td:nth-child(6)').text()).toBe('NAT4')
    expect(rows[3].text()).toContain('connection_path.fallback_relay')
  })

  it('shows the selected transport for direct and relayed traffic with standby links', async () => {
    const wrapper = await render(undefined, undefined, undefined, true, 'connections')
    const instance = wrapper.props('curNetworkInst') as NetworkInstance
    const detail = instance.detail!
    const [node, direct, relayed] = detail.peer_route_pairs
    const conns = [
      { conn_id: '00000000-0000-0000-0000-000000000001', tunnel: { tunnel_type: 'udp' }, stats: { latency_us: '14000' }, loss_rate: 0.12 },
      { conn_id: '00000000-0000-0000-0000-000000000002', tunnel: { tunnel_type: 'tcp' }, stats: { latency_us: '32000' } },
      { conn_id: '00000000-0000-0000-0000-000000000003', tunnel: { tunnel_type: 'faketcp_windivert' }, stats: { latency_us: '50000' } },
    ]
    const update = async (nodeActive: number, directActive: number) => {
      await wrapper.setProps({
        curNetworkInst: {
          ...instance,
          detail: {
            ...detail,
            peer_route_pairs: [
              { ...node, peer: { peer_id: 2, conns, default_conn_id: { part4: nodeActive } } },
              { ...direct, peer: { peer_id: 3, conns, default_conn_id: { part4: directActive } } },
              { ...relayed, route: { ...relayed.route, next_hop_peer_id: 2 }, peer: {
                peer_id: 4, conns: [{ conn_id: 'stale-direct', stats: { latency_us: '5000' }, loss_rate: 0.75 }],
              } },
            ],
          },
        } as NetworkInstance,
      })
    }
    const pathDetails = () => wrapper.findAll('tbody tr').slice(1).map(row => {
      const details = row.find('td:nth-child(3) small')
      return details.exists() ? details.text() : ''
    })
    const quality = () => wrapper.findAll('tbody tr').slice(1).map(row => [row.get('td:nth-child(4)').text(), row.get('td:nth-child(5)').text()])
    await update(2, 1)
    expect(pathDetails()).toEqual(['TCP', 'UDP', 'TCP'])
    expect(quality()).toEqual([['32ms', '0%'], ['14ms', '12%'], ['32ms', '0%']])
    await update(1, 2)
    expect(pathDetails()).toEqual(['UDP', 'TCP', 'UDP'])
    expect(quality()).toEqual([['14ms', '12%'], ['32ms', '0%'], ['14ms', '12%']])
    await update(3, 1)
    expect(pathDetails()).toEqual(['', 'UDP', 'remote-node'])
    expect(quality()).toEqual([['50ms', '0%'], ['14ms', '12%'], ['50ms', '0%']])
  })

  it('shows one connected node and the actual relay address for each relayed device', async () => {
    const addresses = { primary: '198.51.100.10:22021', secondary: '198.51.100.20:22021' }
    const wrapper = await render(undefined, undefined, undefined, true, 'connections', [
      `tcp://${addresses.secondary}`, `udp://${addresses.secondary}`,
    ])
    const instance = wrapper.props('curNetworkInst') as NetworkInstance
    const [node, direct, relayed] = instance.detail!.peer_route_pairs
    const connection = (address: string) => [{
      conn_id: 'connection',
      tunnel: {
        tunnel_type: 'udp',
        remote_addr: { url: `udp://${address}` },
        resolved_remote_addr: { url: `udp://${address}` },
      },
    }]
    const secondary = { ...node, peer: { peer_id: 2, conns: connection(addresses.secondary) } }
    const update = async (primaryConnected: boolean, relayHop: number) => {
      await wrapper.setProps({ curNetworkInst: {
        ...instance,
        detail: {
          ...instance.detail!,
          peer_route_pairs: [
            {
              route: { ...node.route, peer_id: 5, hostname: 'primary-server', cost: primaryConnected ? 1 : 2 },
              peer: primaryConnected ? { peer_id: 5, conns: connection(addresses.primary) } : undefined,
            },
            secondary,
            direct,
            { ...relayed, route: { ...relayed.route, next_hop_peer_id: relayHop } },
          ],
        },
      } as NetworkInstance })
    }
    const names = () => wrapper.findAll('tbody th').map(cell => cell.text())
    const paths = () => wrapper.findAll('tbody tr').slice(2).map(row => row.get('td:nth-child(3)').text())

    await update(false, 2)
    expect(names()).toEqual(['local-node', addresses.secondary, 'direct-device', 'relayed-device'])
    expect(paths()[0]).toBe('connection_path.p2p_direct')
    expect(paths()[1]).toContain(addresses.secondary)

    await update(true, 5)
    expect(names()).toEqual(['local-node', addresses.secondary, 'direct-device', 'relayed-device'])
    expect(paths()[1]).toContain(addresses.primary)
  })

  it('leaves an unclassified NAT blank on desktop', async () => {
    const wrapper = await render({ public_ip: ['192.0.2.2'] }, undefined, undefined, false, 'overview')
    expect(wrapper.findAll('.desk-local dd')[1].text()).toBe('—')
    expect(wrapper.findAll('tbody tr').map(row => row.get('td:nth-child(6)').text())).toEqual(['—', '—'])
  })

  it('opens tray-selected peers and copies a bare IPv4 without losing network diagnostics', async () => {
    const wrapper = await render(undefined, undefined, undefined, true, 'devices')
    expect(wrapper.get('.desk-device-list').text()).not.toContain('remote-node')
    expect(wrapper.get('.desk-device-detail').text()).toContain('direct-device')

    await wrapper.get('.desk-device-search input').setValue('direct-device')
    expect(wrapper.findAll('.desk-device-row')).toHaveLength(1)
    await wrapper.setProps({ requestedPeerId: '4' })
    expect(wrapper.get('.desk-device-detail').text()).toContain('relayed-device')
    expect(wrapper.get('.desk-device-detail').text()).toContain('connection_path.fallback_relay')
    await wrapper.get('.desk-device-copy').trigger('click')
    expect(wrapper.emitted('copyAddress')).toEqual([['10.250.0.3']])

    await wrapper.setProps({ desktopView: 'connections' })
    expect(wrapper.get('.desk-connections').isVisible()).toBe(true)
    expect(wrapper.get('.desk-connections').text()).toContain('remote-node')
    await wrapper.setProps({ desktopView: 'overview' })
    expect(wrapper.get('.desk-traffic').isVisible()).toBe(true)
  })
})
