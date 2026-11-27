import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import RemoteManagement from '../src/components/RemoteManagement.vue'
import {
  DEFAULT_NETWORK_CONFIG,
  type NetworkConfig,
} from '../src/types/network'

const BOOLEAN_CONFIG_FIELDS = [
  'dhcp',
  'advanced_settings',
  'latency_first',
  'use_smoltcp',
  'disable_ipv6',
  'enable_kcp_proxy',
  'disable_kcp_input',
  'disable_p2p',
  'bind_device',
  'no_tun',
  'enable_exit_node',
  'relay_all_peer_rpc',
  'multi_thread',
  'enable_relay_network_whitelist',
  'enable_manual_routes',
  'proxy_forward_by_system',
  'disable_encryption',
  'enable_socks5',
  'disable_udp_hole_punching',
  'enable_magic_dns',
  'enable_private_mode',
  'enable_quic_proxy',
  'disable_quic_input',
  'disable_sym_hole_punching',
  'p2p_only',
  'lazy_p2p',
  'need_p2p',
  'disable_upnp',
  'ipv6_public_addr_provider',
  'ipv6_public_addr_auto',
  'disable_relay_data',
  'enable_udp_broadcast_relay',
  'disable_tcp_hole_punching',
] as const satisfies readonly (keyof NetworkConfig)[]

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

vi.mock('primevue', async () => {
  const { defineComponent, h } = await import('vue')

  const PassThrough = defineComponent({
    name: 'PassThrough',
    props: {
      label: String,
      value: String,
    },
    setup(props, { slots }) {
      return () => h('div', {
        'data-label': props.label,
        'data-value': props.value,
        'data-stub': 'pass-through',
      }, slots.default?.())
    },
  })

  const ButtonStub = defineComponent({
    name: 'Button',
    props: {
      label: String,
      icon: String,
      disabled: Boolean,
    },
    emits: ['click'],
    setup(props, { slots, emit }) {
      return () => h('button', {
        type: 'button',
        disabled: props.disabled,
        'data-label': props.label ?? props.icon,
        onClick: (event: MouseEvent) => emit('click', event),
      }, slots.default?.() ?? props.label ?? props.icon)
    },
  })

  const SelectStub = defineComponent({
    name: 'Select',
    props: {
      modelValue: Object,
      options: Array,
    },
    emits: ['update:modelValue'],
    setup(props, { slots }) {
      return () => h('div', { 'data-stub': 'select' }, [
        slots.value?.({ value: props.modelValue, placeholder: '' }),
      ])
    },
  })

  const MenuStub = defineComponent({
    name: 'Menu',
    setup(_, { expose }) {
      expose({ toggle: vi.fn() })
      return () => h('div', { 'data-stub': 'menu' })
    },
  })

  return {
    Button: ButtonStub,
    ConfirmPopup: PassThrough,
    Divider: PassThrough,
    IftaLabel: PassThrough,
    Menu: MenuStub,
    Message: PassThrough,
    Select: SelectStub,
    Tag: PassThrough,
    useConfirm: () => ({ require: vi.fn() }),
    useToast: () => ({ add: vi.fn() }),
  }
})

const INSTANCE_ID = '00000000-0000-0000-0000-000000000001'
const INSTANCE_UUID = {
  part1: 0,
  part2: 0,
  part3: 0,
  part4: 1,
}

function makeFlagConfig(): NetworkConfig {
  const config = {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: INSTANCE_ID,
    network_name: 'mesh-save',
  }

  BOOLEAN_CONFIG_FIELDS.forEach((field, index) => {
    config[field] = index % 2 === 0
  })

  return config
}

function cloneConfig(config: NetworkConfig): NetworkConfig {
  return JSON.parse(JSON.stringify(config)) as NetworkConfig
}

function snapshotBooleanConfigFields(config: NetworkConfig): Record<string, unknown> {
  return Object.fromEntries(
    BOOLEAN_CONFIG_FIELDS.map((field) => [field, config[field]]),
  )
}

async function settleRemoteManagement() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0))
    await flushPromises()
    await nextTick()
  }
}

describe('RemoteManagement config save', () => {
  it('keeps an assigned network selectable and its switch usable while the desktop tunnel is paused', async () => {
    let running = false
    const getNetworkInfo = vi.fn(async () => ({ running: true }))
    const updateNetwork = vi.fn()
    const api = {
      list_network_instance_ids: vi.fn(async () => ({ running_inst_ids: running ? [INSTANCE_UUID] : [], disabled_inst_ids: [] })),
      get_network_info: getNetworkInfo,
      get_network_metas: vi.fn(async () => ({ metas: {} })),
      update_network_instance_state: updateNetwork,
    } as unknown as import('../src/modules/api').RemoteClient
    const wrapper = mount(RemoteManagement, {
      props: {
        api, managed: true, desktopView: 'devices', instanceId: INSTANCE_ID,
        assignedNetworks: [{ network_id: INSTANCE_ID, display_name: '办公室网络' }],
        desktopConnection: { enabled: false, busy: false, available: true },
      },
      global: { stubs: { Config: true, ConfigEditDialog: true, Status: true } },
    })

    try {
      await settleRemoteManagement()
      const toggle = wrapper.get('[role="switch"]')
      expect(wrapper.text()).toContain('办公室网络')
      expect(toggle.attributes('aria-checked')).toBe('false')
      expect(toggle.attributes('disabled')).toBeUndefined()
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await toggle.trigger('click')
      expect(wrapper.emitted('toggleManagedConnection')).toHaveLength(1)
      expect(updateNetwork).not.toHaveBeenCalled()

      await wrapper.setProps({ desktopConnection: { enabled: true, busy: true, available: true } })
      expect(toggle.attributes('disabled')).toBeDefined()
      expect(wrapper.get('.desktop-workspace-network-state').text()).toBe('desktop.connecting')

      running = true
      await wrapper.setProps({ desktopConnection: { enabled: true, busy: false, available: true } })
      await wrapper.vm.refreshDesktopNetwork()
      expect(wrapper.get('.desktop-workspace-network-state').text()).toBe('desktop.connected')
      expect(toggle.attributes('aria-checked')).toBe('true')
    } finally {
      wrapper.unmount()
    }
  })

  it('selects a newly started assigned network and hides local management', async () => {
    let joined = false
    const api = {
      list_network_instance_ids: vi.fn(async () => ({
        running_inst_ids: joined ? [INSTANCE_UUID] : [],
        disabled_inst_ids: [{ ...INSTANCE_UUID, part4: 2 }],
      })),
      get_network_info: vi.fn(async () => ({ running: true })),
      get_network_metas: vi.fn(async () => ({ metas: {} })),
    } as unknown as import('../src/modules/api').RemoteClient
    const wrapper = mount(RemoteManagement, {
      props: { api, managed: true, assignedNetworks: [{ network_id: INSTANCE_ID, display_name: '后台测试网' }] },
      global: { stubs: { Config: true, ConfigEditDialog: true, Status: true } },
    })

    try {
      await settleRemoteManagement()
      expect(wrapper.text()).toContain('controller.applying_assignment')
      expect(wrapper.find('button[data-label="pi-plus"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.create_network"]').exists()).toBe(false)
      joined = true
      await vi.waitFor(() => {
        expect(wrapper.emitted('update:instanceId')).toContainEqual([INSTANCE_ID])
      }, { timeout: 2500 })
      await wrapper.setProps({ instanceId: INSTANCE_ID })
      expect(wrapper.text()).toContain('后台测试网')
      expect(wrapper.text()).not.toContain(INSTANCE_ID)
      await wrapper.setProps({ assignedNetworks: [] })
      expect(wrapper.text()).toContain('controller.assigned_network')
      expect(wrapper.text()).not.toContain(INSTANCE_ID)
      expect(wrapper.find('button[data-label="web.device_management.disable_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.more_actions"]').exists()).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })

  it('saves the current network config without dropping boolean fields', async () => {
    const config = makeFlagConfig()
    const expectedFlags = snapshotBooleanConfigFields(config)
    const api = {
      delete_network: vi.fn(),
      generate_config: vi.fn(),
      get_network_config: vi.fn(async () => cloneConfig(config)),
      get_network_info: vi.fn(),
      get_vpn_portal_info: vi.fn(),
      get_network_metas: vi.fn(async (instanceIds: string[]) => ({
        metas: Object.fromEntries(instanceIds.map((id) => [id, {
          config_permission: 0xffffffff,
          inst_id: INSTANCE_UUID,
          instance_name: 'mesh-save',
          network_name: 'mesh-save',
          source: 2,
        }])),
      })),
      list_network_instance_ids: vi.fn(async () => ({
        disabled_inst_ids: [INSTANCE_UUID],
        running_inst_ids: [],
      })),
      parse_config: vi.fn(),
      run_network: vi.fn(),
      save_config: vi.fn(async () => undefined),
      update_network_instance_state: vi.fn(),
      validate_config: vi.fn(),
    }

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const saveButton = wrapper.find('button[data-label="web.device_management.save_config"]')
      expect(saveButton.exists()).toBe(true)
      expect(saveButton.attributes('disabled')).toBeUndefined()

      await saveButton.trigger('click')
      await flushPromises()

      expect(api.save_config).toHaveBeenCalledOnce()
      const savedConfig = api.save_config.mock.calls[0][0] as NetworkConfig

      for (const field of BOOLEAN_CONFIG_FIELDS) {
        expect(savedConfig[field], `${field} should be saved`).toBe(expectedFlags[field])
      }
    } finally {
      wrapper.unmount()
    }
  })
})
