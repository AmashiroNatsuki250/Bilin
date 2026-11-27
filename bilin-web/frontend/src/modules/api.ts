import axios, { type AxiosError, type AxiosInstance } from 'axios';
import { Md5 } from 'ts-md5';

interface ConsoleInfo {
    username: string;
    platform_admin: boolean;
}

export interface DeviceLoginDetails {
    device_code: string;
    hostname: string;
}

export interface CentralNetworkSummary {
    network_id: string;
    display_name: string;
    network_name: string;
    networking_method: string;
    virtual_cidr?: string | null;
    secure_mode?: boolean;
    member_count: number;
    online_member_count: number;
}

export interface CentralNetworkMember {
    member_id: string;
    device_id: string;
    hostname: string | null;
    hostname_override: string | null;
    alias?: string | null;
    virtual_ipv4: string | null;
    allocated_ipv4?: string | null;
    online: boolean;
    running: boolean | null;
    runtime_virtual_ipv4: string | null;
    version: string | null;
    error_msg: string | null;
    has_override?: boolean;
    proxy_cidrs?: string[];
    temporary?: boolean;
    credential_id?: string | null;
    credential_expiry_unix?: number | null;
}

export interface ControllerDevice {
    device_id: string;
    device_code: string;
    owner_id: number | null;
    hostname: string;
    version: string;
    first_seen: number;
    last_seen: number;
    online: boolean;
    config_connected: boolean;
    upstream_subnet_cidr?: string | null;
    assignments: { network_id: string; display_name: string; gateway: string | null; path?: string }[];
}

export type PlatformNetwork = CentralNetworkSummary & { owner_id: number };
export type ForcedProtocol = 'auto' | 'udp' | 'kcp' | 'quic' | 'tcp' | 'wss';
export interface PlatformUser {
    id: number;
    username: string;
    banned: boolean;
    udp_enabled: boolean;
    force_relay: boolean;
    forced_protocol: string;
}

export type UserPolicyPatch = Partial<Pick<PlatformUser, 'banned' | 'udp_enabled' | 'force_relay'> & { forced_protocol: ForcedProtocol }>;

export interface ControllerOverview {
    users: { id: number; username: string }[];
    networks: PlatformNetwork[];
    devices: ControllerDevice[];
    gateway: string | null;
    gateway_forwarding: boolean;
    forwarding_nodes: { id: string; name: string; peer_url: string; online: boolean; ready: boolean; last_seen: number | null }[];
}

export default class ApiClient {
    private readonly client: AxiosInstance;

    constructor(baseUrl: string, onAuthFailure?: () => void) {
        this.client = axios.create({
            baseURL: baseUrl.replace(/\/+$/, '') + '/api/v1',
            withCredentials: true,
            headers: { 'Content-Type': 'application/json' },
        });
        this.client.interceptors.response.use(response => response.data, (error: AxiosError) => {
            if (error.response?.status === 401) onAuthFailure?.();
            return Promise.reject(error);
        });
    }

    public async login(username: string, password: string): Promise<void> {
        await this.client.post('/auth/login', { username, password: Md5.hashStr(password) });
    }

    public device_login_details(ticket: string): Promise<DeviceLoginDetails> {
        return this.client.post<unknown, DeviceLoginDetails>('/enrollment/login/details', { ticket });
    }

    public async approve_device_login(ticket: string): Promise<void> {
        await this.client.post('/workspace/device-login', { ticket });
    }

    public async register(username: string, password: string, captcha: string): Promise<void> {
        await this.client.post('/auth/register', {
            credentials: { username, password: Md5.hashStr(password) },
            captcha,
        });
    }

    public captchaUrl() {
        return this.client.getUri({ url: '/auth/captcha' });
    }

    public async logout() {
        await this.client.get('/auth/logout');
    }

    public async change_password(newPassword: string) {
        await this.client.put('/auth/password', { new_password: Md5.hashStr(newPassword) });
    }

    public get_console_info(): Promise<ConsoleInfo> {
        return this.client.get<unknown, ConsoleInfo>('/console-info');
    }

    public async list_networks(): Promise<CentralNetworkSummary[]> {
        const response = await this.client.get<unknown, { networks: CentralNetworkSummary[] }>('/networks');
        return response.networks;
    }

    public controller_devices(): Promise<ControllerDevice[]> {
        return this.client.get<unknown, ControllerDevice[]>('/workspace/devices');
    }

    public async claim_controller_device(device_id: string, pairing_code?: string): Promise<void> {
        await this.client.post('/workspace/devices/claim', { device_id, ...(pairing_code ? { pairing_code } : {}) });
    }

    public async unbind_controller_device(deviceId: string): Promise<void> {
        await this.client.delete(`/workspace/devices/${deviceId}`);
    }

    public async delete_controller_device(code: string, admin: boolean, confirm: boolean): Promise<void> {
        const id = encodeURIComponent(code);
        const url = admin ? `/admin/devices/${id}` : `/workspace/devices/${id}/registration`;
        await this.client.delete(url, { params: { confirm } });
    }

    public controller_overview(): Promise<ControllerOverview> {
        return this.client.get<unknown, ControllerOverview>('/admin/overview');
    }

    public async platform_users(): Promise<PlatformUser[]> {
        const response = await this.client.get<unknown, { users: PlatformUser[] }>('/admin/users');
        return response.users;
    }

    public set_user_policy(userId: number, policy: UserPolicyPatch): Promise<PlatformUser> {
        return this.client.patch<unknown, PlatformUser>(`/admin/users/${userId}/policy`, policy);
    }

    public async delete_platform_user(userId: number): Promise<void> {
        await this.client.delete(`/admin/users/${userId}`, { params: { confirm: true } });
    }

    public create_forwarding_node(name: string, peer_url: string): Promise<{ id: string; registration_token: string }> {
        return this.client.post<unknown, { id: string; registration_token: string }>('/admin/nodes', { name, peer_url });
    }

    public async bind_controller_device(deviceId: string, owner_id: number): Promise<void> {
        await this.client.post(`/admin/devices/${deviceId}/bind`, { owner_id });
    }

    public async rename_controller_device(deviceId: string, name: string, admin: boolean): Promise<void> {
        const portal = admin ? 'admin' : 'workspace';
        await this.client.patch(`/${portal}/devices/${encodeURIComponent(deviceId)}/name`, { name });
    }

    public async set_controller_device_upstream_subnet(deviceId: string, upstream_subnet_cidr: string | null): Promise<void> {
        await this.client.patch(`/admin/devices/${encodeURIComponent(deviceId)}/upstream-subnet`, { upstream_subnet_cidr });
    }

    public create_controller_network(display_name: string, owner?: number): Promise<{ network_id: string }> {
        const url = owner === undefined ? '/workspace/networks' : `/admin/users/${owner}/networks`;
        return this.client.post<unknown, { network_id: string }>(url, { display_name });
    }

    public async controller_members(networkId: string, owner?: number): Promise<CentralNetworkMember[]> {
        const url = owner === undefined ? `/networks/${networkId}/members` : `/admin/users/${owner}/networks/${networkId}/devices`;
        const response = await this.client.get<unknown, { members: CentralNetworkMember[] }>(url);
        return response.members;
    }

    public async add_controller_member(networkId: string, device_id: string, owner?: number): Promise<void> {
        const prefix = owner === undefined ? '/workspace' : `/admin/users/${owner}`;
        await this.client.post(`${prefix}/networks/${networkId}/devices`, { device_id });
    }

    public async add_controller_member_by_code(networkId: string, device_id: string): Promise<void> {
        await this.client.post(`/workspace/networks/${encodeURIComponent(networkId)}/devices/by-code`, { device_id });
    }

    public async remove_controller_member(networkId: string, deviceId: string, owner?: number): Promise<void> {
        const url = owner === undefined ? `/networks/${networkId}/members/${deviceId}`
            : `/admin/users/${owner}/networks/${networkId}/devices/${deviceId}`;
        await this.client.delete(url);
    }

    public async delete_controller_network(networkId: string, owner?: number): Promise<void> {
        const prefix = owner === undefined ? '' : `/admin/users/${owner}`;
        await this.client.delete(`${prefix}/networks/${networkId}`);
    }
}
