import { describe, expect, it } from 'vitest'
import { deviceLoginStorageKey, updateDeviceLoginAttempt } from './device_login'

describe('device login state', () => {
  const pending = { revision: 1, bound: false }
  const unbound = { registered: true, bound: false, device_login_revision: 1 }

  it('keeps the pending attempt across offline or initial provisional status', () => {
    expect(updateDeviceLoginAttempt(pending, { registered: false, bound: false })).toBe(pending)
    expect(updateDeviceLoginAttempt(pending, unbound)).toBe(pending)
    expect(updateDeviceLoginAttempt(pending, { ...unbound, device_login_revision: 0 })).toBe(pending)
  })

  it('records binding confirmation and resets the entry after Web removes the device', () => {
    const bound = updateDeviceLoginAttempt(pending, { ...unbound, bound: true })
    expect(bound).toEqual({ revision: 1, bound: true })
    expect(updateDeviceLoginAttempt(bound, { registered: false, bound: false })).toBe(bound)
    expect(updateDeviceLoginAttempt(bound, { ...unbound, device_login_revision: 2 })).toBeNull()
    expect(updateDeviceLoginAttempt(pending, { ...unbound, device_login_revision: 2 })).toBeNull()
  })

  it('does not share the attempt across different device or controller identities', () => {
    const status = { ...unbound, device_id: 'device-1', controller_origin: 'https://control.example/', connected: false, networks: [] }
    const key = deviceLoginStorageKey(status)
    expect(deviceLoginStorageKey({ ...status, device_id: 'device-2' })).not.toBe(key)
    expect(deviceLoginStorageKey({ ...status, controller_origin: 'https://other.example/' })).not.toBe(key)
  })
})
