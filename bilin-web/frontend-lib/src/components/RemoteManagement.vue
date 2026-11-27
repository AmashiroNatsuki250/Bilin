<script setup lang="ts">
import { Button, ConfirmDialog, ConfirmPopup, Divider, IftaLabel, Menu, Message, Select, Tag, useConfirm, useToast, type VirtualScrollerLazyEvent } from 'primevue';
import { computed, onMounted, onUnmounted, Ref, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import * as Api from '../modules/api';
import * as Utils from '../modules/utils';
import * as NetworkTypes from '../types/network';
import { type MenuItem } from 'primevue/menuitem';
import DesktopConnectionSwitch from './DesktopConnectionSwitch.vue';

const { t } = useI18n()

const props = defineProps<{
    api: Api.RemoteClient;
    newConfigGenerator?: () => NetworkTypes.NetworkConfig;
    pauseAutoRefresh?: boolean;
    managed?: boolean;
    assignedNetworks?: { network_id: string; display_name: string }[];
    desktopView?: 'overview' | 'connections' | 'devices';
    desktopConnection?: { enabled: boolean; busy: boolean; available: boolean };
    requestedPeerId?: string | null;
}>();

const instanceId = defineModel('instanceId', {
    type: String as () => string | undefined,
    required: false,
})

const emits = defineEmits<{
    update: [];
    copyAddress: [address: string];
    devicePanelState: [state: NetworkTypes.DesktopPanelState];
    toggleManagedConnection: [];
}>();

const toast = useToast();

const configFile = ref();

const curNetworkInfo = ref<NetworkTypes.NetworkInstance | null>(null);

const desktopDevicePeers = ref<NetworkTypes.DesktopPanelPeer[]>([]);

const showConfigEditDialog = ref(false);
const isEditingNetwork = ref(false); // Flag to indicate if we're in network editing mode
const currentNetworkConfig = ref<NetworkTypes.NetworkConfig | undefined>(undefined);

const listInstanceIdResponse = ref<Api.ListNetworkInstanceIdResponse | undefined>(undefined);

const isRunning = (instanceId: string) => {
    return (listInstanceIdResponse.value?.running_inst_ids ?? []).map(Utils.UuidToStr).includes(instanceId);
}

const networkMetaCache = ref<Record<string, Api.NetworkMeta>>({});
const loadNetworkMetas = async (instanceIds: string[]) => {
    const missingIds = instanceIds.filter(id => !networkMetaCache.value[id]);

    if (missingIds.length === 0) return;

    try {
        const response = await props.api.get_network_metas(missingIds);
        Object.assign(networkMetaCache.value, response.metas ?? {});
    } catch (e) {
        console.error("Failed to load network metas", e);
    }
};
const onLazyLoadNetworkMetas = async (event: VirtualScrollerLazyEvent) => {
    const instanceIds = instanceList.value
        .slice(event.first, event.last + 1)
        .map(item => item.uuid);
    await loadNetworkMetas(instanceIds);
};
const currentNetworkMeta = computed(() => {
    if (!instanceId.value) {
        return undefined;
    }
    return networkMetaCache.value[instanceId.value];
});
const currentNetworkControl = {
    remoteSave: computed(() => {
        return Api.ConfigFilePermission.isRemoveSaveable(currentNetworkMeta.value?.config_permission ?? 0);
    }),
    editable: computed(() => {
        return Api.ConfigFilePermission.isEditable(currentNetworkMeta.value?.config_permission ?? 0);
    }),
    deletable: computed(() => {
        return Api.ConfigFilePermission.isDeletable(currentNetworkMeta.value?.config_permission ?? 0);
    })
}

const instanceList = ref<Array<{ uuid: string; meta?: Api.NetworkMeta }>>([]);
const assignedNames = computed(() => new Map(
    props.assignedNetworks?.map(network => [network.network_id, network.display_name])
));
const networkName = (option: { uuid: string; meta?: Api.NetworkMeta }) =>
    assignedNames.value.get(option.uuid) || (props.managed
        ? t('controller.assigned_network')
        : option.meta?.network_name || option.uuid);

const updateInstanceList = () => {
    let insts = new Set<string>();
    let t = listInstanceIdResponse.value;
    if (t) {
        (t.running_inst_ids ?? []).forEach((u) => insts.add(Utils.UuidToStr(u)));
        if (!props.managed) {
            (t.disabled_inst_ids ?? []).forEach((u) => insts.add(Utils.UuidToStr(u)));
        }
    }
    if (props.managed && props.desktopConnection) {
        props.assignedNetworks?.forEach(network => insts.add(network.network_id));
    }

    const newList = Array.from(insts).map((instance: string) => {
        return {
            uuid: instance,
            meta: networkMetaCache.value[instance]
        };
    });

    if (JSON.stringify(newList) !== JSON.stringify(instanceList.value)) {
        instanceList.value = newList;
    }
    if (props.managed && !insts.has(instanceId.value ?? '')) {
        instanceId.value = newList[0]?.uuid;
    }
}
watch(listInstanceIdResponse, updateInstanceList, { deep: false });
watch(networkMetaCache, updateInstanceList, { deep: true });
watch(() => props.managed, updateInstanceList);
watch([() => !!props.desktopConnection, assignedNames], updateInstanceList);
watch(instanceList, async (newVal) => {
    if (newVal) {
        const instanceIds = new Set(newVal.map(item => item.uuid));
        Object.keys(networkMetaCache.value).forEach(id => {
            if (!instanceIds.has(id)) {
                delete networkMetaCache.value[id];
            }
        });
    }
});

const selectedInstanceId = computed({
    get() {
        return instanceList.value.find((instance) => instance.uuid === instanceId.value);
    },
    set(value: any) {
        console.log("set instanceId", value);
        instanceId.value = value ? value.uuid : undefined;
    }
});
const desktopCurrentNetwork = computed(() => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected || (props.desktopConnection && !isRunning(selected))) return null;
    return curNetworkInfo.value?.instance_id === selected ? curNetworkInfo.value : null;
});
const desktopConnectionLabel = computed(() => desktopCurrentNetwork.value?.running ? 'desktop.connected'
    : props.desktopConnection?.available && props.desktopConnection.enabled ? 'desktop.connecting' : 'desktop.disconnected');
const desktopNetworkAddress = computed(() => {
    const address = desktopCurrentNetwork.value?.detail?.my_node_info?.virtual_ipv4;
    return (typeof address === 'string' ? address : address?.address && Utils.ipv4InetToString(address))?.split('/')[0] ?? '';
});
watch(selectedInstanceId, async (newVal, oldVal) => {
    if (newVal?.uuid !== oldVal?.uuid && (networkIsDisabled.value || isEditingNetwork.value)) {
        await loadCurrentNetworkConfig();
    } else {
        await loadCurrentNetworkInfo();
    }

    if (newVal?.uuid && !props.desktopConnection && !networkMetaCache.value[newVal.uuid]) {
        await loadNetworkMetas([newVal.uuid]);
    }
});

const needShowNetworkStatus = computed(() => {
    if (!selectedInstanceId.value) {
        // nothing selected
        return false;
    }
    if (props.desktopConnection && !isRunning(selectedInstanceId.value.uuid)) {
        return false;
    }
    if (networkIsDisabled.value) {
        // network is disabled
        return false;
    }
    if (isEditingNetwork.value) {
        // editing network
        return false;
    }
    return true;
})

const networkIsDisabled = computed(() => {
    if (!selectedInstanceId.value) {
        return false;
    }
    return (listInstanceIdResponse.value?.disabled_inst_ids ?? []).map(Utils.UuidToStr).includes(selectedInstanceId.value?.uuid);
});
watch(networkIsDisabled, async (newVal, oldVal) => {
    if (newVal !== oldVal && newVal === true) {
        await loadCurrentNetworkConfig();
    }
});

const loadCurrentNetworkConfig = async () => {
    currentNetworkConfig.value = undefined;

    if (!selectedInstanceId.value) {
        return;
    }

    let ret = await props.api.get_network_config(selectedInstanceId.value!.uuid);
    currentNetworkConfig.value = ret;
}

const changingNetwork = ref(false);
const canToggleDesktopNetwork = computed(() => !props.managed && Boolean(selectedInstanceId.value)
    && currentNetworkControl.deletable.value && !isEditingNetwork.value && !changingNetwork.value
    && (networkIsDisabled.value || Boolean(desktopCurrentNetwork.value?.running)));

const setSelectedNetworkDisabled = async (disabled: boolean) => {
    const selected = selectedInstanceId.value;
    if (!selected || props.managed || changingNetwork.value) return;

    changingNetwork.value = true;
    try {
        await props.api.update_network_instance_state(selected.uuid, disabled);
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
    } catch (error) {
        toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 4000 });
    } finally {
        changingNetwork.value = false;
    }
}
const stopNetwork = () => setSelectedNetworkDisabled(true);
const connectCurrentNetwork = () => canToggleDesktopNetwork.value && networkIsDisabled.value
    ? setSelectedNetworkDisabled(false) : undefined;
const disconnectCurrentNetwork = () => canToggleDesktopNetwork.value && !networkIsDisabled.value
    ? stopNetwork() : undefined;
const toggleCurrentNetwork = () => networkIsDisabled.value ? connectCurrentNetwork() : disconnectCurrentNetwork();
const refreshDesktopNetwork = async () => {
    await loadNetworkInstanceIds();
    await loadCurrentNetworkInfo();
};
defineExpose({ connectCurrentNetwork, disconnectCurrentNetwork, refreshDesktopNetwork });

watch(() => curNetworkInfo.value?.instance_id, () => {
    desktopDevicePeers.value = [];
});
watch(() => props.desktopView && {
    networkName: selectedInstanceId.value ? networkName(selectedInstanceId.value) : '',
    address: desktopNetworkAddress.value,
    connected: Boolean(desktopCurrentNetwork.value?.running),
    canToggle: canToggleDesktopNetwork.value,
    peers: desktopCurrentNetwork.value?.running ? desktopDevicePeers.value : [],
}, (state) => {
    if (state) emits('devicePanelState', state);
}, { immediate: true });

const confirm = useConfirm();
const confirmDeleteNetwork = (event?: Event) => {
    const selected = instanceId.value;
    if (!selected) return;
    confirm.require({
        group: props.desktopView ? 'desktop-network' : undefined,
        target: event?.currentTarget as HTMLElement | undefined,
        header: t('web.device_management.delete_network'),
        message: t('controller.confirm_delete_network', { name: selectedInstanceId.value ? networkName(selectedInstanceId.value) : selected }),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true
        },
        acceptProps: {
            label: t('web.common.delete'),
            severity: 'danger'
        },
        accept: async () => {
            try {
                await props.api.delete_network(selected);
                await loadNetworkInstanceIds();
            } catch (e) {
                console.error(e);
            }
            emits('update');
        },
        reject: () => {
            return;
        }
    });
};

const saveAndRunNewNetwork = async (config?: NetworkTypes.NetworkConfig) => {
    const cfg = config ?? currentNetworkConfig.value;
    if (!cfg) {
        return;
    }

    const targetInstanceId = instanceId.value ?? cfg.instance_id;
    if (targetInstanceId && cfg.instance_id !== targetInstanceId) {
        cfg.instance_id = targetInstanceId;
    }

    try {
        if (networkIsDisabled.value) {
            await props.api.save_config(cfg);
            await props.api.update_network_instance_state(cfg.instance_id, false);
        } else {
            await props.api.run_network(cfg, currentNetworkControl.remoteSave.value);
        }

        delete networkMetaCache.value[cfg.instance_id];
        await loadNetworkMetas([cfg.instance_id]);

        selectedInstanceId.value = { uuid: cfg.instance_id };
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to run network, error: ' + JSON.stringify(e.response?.data ?? e), life: 2000 });
        return;
    }

    emits('update');
    isEditingNetwork.value = false;
}

const saveNetworkConfig = async () => {
    if (!currentNetworkConfig.value) {
        return;
    }
    await props.api.save_config(currentNetworkConfig.value);

    delete networkMetaCache.value[currentNetworkConfig.value.instance_id];
    await loadNetworkMetas([currentNetworkConfig.value.instance_id]);

    toast.add({ severity: 'success', summary: t("web.common.success"), detail: t("web.device_management.config_saved"), life: 2000 });
}
const newNetwork = async () => {
    const newNetworkConfig = props.newConfigGenerator?.() ?? NetworkTypes.DEFAULT_NETWORK_CONFIG();
    await props.api.save_config(newNetworkConfig);
    selectedInstanceId.value = { uuid: newNetworkConfig.instance_id };
    currentNetworkConfig.value = newNetworkConfig;
    await loadNetworkInstanceIds();
}

const cancelEditNetwork = () => {
    isEditingNetwork.value = false;
}

const editNetwork = async () => {
    if (!instanceId.value) {
        toast.add({ severity: 'error', summary: 'Error', detail: 'No network instance selected', life: 2000 });
        return;
    }

    try {
        let ret = await props.api.get_network_config(instanceId.value!);
        console.debug("editNetwork", ret);
        currentNetworkConfig.value = ret;
        isEditingNetwork.value = true; // Switch to editing mode instead
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to edit network, error: ' + JSON.stringify(e.response.data), life: 2000 });
        return;
    }
}

const loadNetworkInstanceIds = async () => {
    const response = await props.api.list_network_instance_ids();
    listInstanceIdResponse.value = response;
    if (props.desktopView && !props.managed) {
        const ids = [...(response.running_inst_ids ?? []), ...(response.disabled_inst_ids ?? [])];
        await loadNetworkMetas(ids.map(Utils.UuidToStr));
    }
}

const loadCurrentNetworkInfo = async () => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected) {
        curNetworkInfo.value = null;
        return;
    }
    if (!needShowNetworkStatus.value) {
        curNetworkInfo.value = null;
        return;
    }
    if (curNetworkInfo.value?.instance_id !== selected) {
        curNetworkInfo.value = null;
    }

    let network_info = await props.api.get_network_info(selected);
    if (selectedInstanceId.value?.uuid !== selected) {
        return;
    }

    if (!network_info) {
        curNetworkInfo.value = {
            instance_id: selected,
            running: false,
            error_msg: t('web.device_management.network_info_unavailable'),
        } as NetworkTypes.NetworkInstance;
        return;
    }

    curNetworkInfo.value = {
        instance_id: selected,
        running: network_info?.running ?? false,
        error_msg: network_info?.error_msg ?? '',
        detail: network_info,
    } as NetworkTypes.NetworkInstance;
}

const exportConfig = async () => {
    if (!instanceId.value) {
        toast.add({ severity: 'error', summary: 'Error', detail: 'No network instance selected', life: 2000 });
        return;
    }

    try {
        const { instance_id, ...networkConfig } = await props.api.get_network_config(instanceId.value!);
        let { toml_config: tomlConfig, error } = await props.api.generate_config(networkConfig as NetworkTypes.NetworkConfig);
        if (error) {
            throw { response: { data: error } };
        }
        exportTomlFile(tomlConfig ?? '', instanceId.value + '.toml');
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to export network config, error: ' + JSON.stringify(e.response.data), life: 2000 });
        return;
    }
}

const importConfig = () => {
    configFile.value.click();
}

const handleFileUpload = (event: Event) => {
    const files = (event.target as HTMLInputElement).files;
    const file = files ? files[0] : null;
    if (!file) return;
    const reader = new FileReader();
    reader.onload = async (e) => {
        try {
            let tomlConfig = e.target?.result?.toString();
            if (!tomlConfig) return;
            const resp = await props.api.parse_config(tomlConfig);
            if (resp.error) {
                throw resp.error;
            }

            const config = resp.config;
            if (!config) return;

            config.instance_id = currentNetworkConfig.value?.instance_id ?? config?.instance_id;
            currentNetworkConfig.value = config;
            toast.add({ severity: 'success', summary: 'Import Success', detail: "Config file import success", life: 2000 });
        } catch (error) {
            toast.add({ severity: 'error', summary: 'Error', detail: 'Config file parse error: ' + error, life: 2000 });
        }
        configFile.value.value = null;
    }
    reader.readAsText(file);
}

const exportTomlFile = (context: string, name: string) => {
    let url = window.URL.createObjectURL(new Blob([context], { type: 'application/toml' }));
    let link = document.createElement('a');
    link.style.display = 'none';
    link.href = url;
    link.setAttribute('download', name);
    document.body.appendChild(link);
    link.click();

    document.body.removeChild(link);
    window.URL.revokeObjectURL(url);
}

const generateConfig = async (config: NetworkTypes.NetworkConfig): Promise<string> => {
    let { toml_config: tomlConfig, error } = await props.api.generate_config(config);
    if (error) {
        throw error;
    }
    return tomlConfig ?? '';
}

const syncTomlConfig = async (tomlConfig: string): Promise<void> => {
    let resp = await props.api.parse_config(tomlConfig);
    if (resp.error) {
        throw resp.error;
    };
    const config = resp.config;
    if (!config) {
        throw new Error("Parsed config is empty");
    }
    config.instance_id = currentNetworkConfig.value?.instance_id ?? config?.instance_id;
    currentNetworkConfig.value = config;
}

// 响应式屏幕宽度
const screenWidth = ref(window.innerWidth);
const updateScreenWidth = () => {
    screenWidth.value = window.innerWidth;
};

// 菜单引用和菜单项
const menuRef = ref();
const actionMenu: Ref<MenuItem[]> = ref([
    {
        label: () => t('web.device_management.edit_network'),
        icon: 'pi pi-pencil',
        visible: () => !(networkIsDisabled.value ?? true) && currentNetworkControl.editable.value,
        command: () => editNetwork()
    },
    {
        label: () => t('web.device_management.export_config'),
        icon: 'pi pi-download',
        command: () => exportConfig()
    },
    {
        label: () => t('web.device_management.delete_network'),
        icon: 'pi pi-trash',
        class: 'p-error',
        visible: () => currentNetworkControl.deletable.value,
        command: event => confirmDeleteNetwork(event.originalEvent)
    }
]);

let periodFunc = new Utils.PeriodicTask(async () => {
    if (props.pauseAutoRefresh) {
        return;
    }
    try {
        await Promise.all([loadNetworkInstanceIds(), loadCurrentNetworkInfo()]);
    } catch (e) {
        console.debug(e);
    }
}, 1000);

onMounted(async () => {
    periodFunc.start();

    // 添加屏幕尺寸监听
    window.addEventListener('resize', updateScreenWidth);
});

onUnmounted(() => {
    periodFunc.stop();

    // 移除屏幕尺寸监听
    window.removeEventListener('resize', updateScreenWidth);
});

</script>


<template>
    <div class="device-management" :class="{ 'desktop-management': desktopView }">
        <input type="file" @change="handleFileUpload" class="hidden" accept="application/toml" ref="configFile" />
        <ConfirmDialog v-if="desktopView && !managed" group="desktop-network" />
        <ConfirmPopup v-else-if="!desktopView" />

        <div v-if="desktopView && (instanceList.length > 1 || !managed)" class="desktop-network-toolbar">
            <div class="desktop-network-select">
                <label for="active-network">{{ t('desktop.network') }}</label>
                <Select v-model="selectedInstanceId" :options="instanceList" :option-label="networkName"
                    input-id="active-network" :placeholder="t('web.device_management.select_network')" />
            </div>
            <div v-if="!managed" class="desktop-network-actions">
                <Button v-if="isEditingNetwork" @click="cancelEditNetwork" icon="pi pi-times"
                    :label="t('web.device_management.cancel_edit')" outlined />
                <template v-else>
                    <Menu ref="menuRef" :model="actionMenu" :popup="true" />
                    <Button v-if="selectedInstanceId" @click="menuRef?.toggle($event)" icon="pi pi-ellipsis-h"
                        :aria-label="t('web.device_management.more_actions')" outlined />
                    <Button @click="newNetwork" icon="pi pi-plus" :label="t('web.device_management.create_network')" />
                </template>
            </div>
        </div>

        <div v-if="!desktopView" class="network-header mb-3">
            <div class="flex flex-row justify-between items-center gap-2" style="align-items: center;">
                <!-- 网络选择 -->
                <div class="flex-1 min-w-0">
                    <IftaLabel class="w-full">
                        <Select v-model="selectedInstanceId" :options="instanceList" :optionLabel="networkName" class="w-full"
                            inputId="dd-inst-id" :placeholder="t('web.device_management.select_network')"
                            :pt="{ root: { class: 'network-select-container' } }" :virtualScrollerOptions="{
                                lazy: true,
                                onLazyLoad: onLazyLoadNetworkMetas,
                                itemSize: 60,
                                delay: 50
                            }">
                            <template #value="slotProps">
                                <div v-if="slotProps.value" class="flex items-center content-center min-w-0">
                                    <div class="mr-4 flex-col min-w-0 flex-1">
                                        <span class="truncate block">
                                            &nbsp;
                                            <span v-if="slotProps.value.meta">
                                                {{ networkName(slotProps.value) }}
                                            </span>
                                            <span v-else>
                                                {{ networkName(slotProps.value) }}
                                            </span>
                                        </span>
                                    </div>
                                    <Tag class="my-auto leading-3 shrink-0"
                                        :severity="isRunning(slotProps.value.uuid) ? 'success' : 'info'"
                                        :value="t(isRunning(slotProps.value.uuid) ? 'network_running' : 'network_stopped')" />
                                </div>
                                <span v-else>
                                    {{ slotProps.placeholder }}
                                </span>
                            </template>
                            <template #option="slotProps">
                                <div class="flex flex-col items-start content-center max-w-full">
                                    <div class="flex items-center min-w-0">
                                        <div class="mr-4 min-w-0 flex-1">
                                            <span class="truncate block">{{ t('network_name') }}: {{
                                                networkName(slotProps.option) }}</span>
                                        </div>
                                        <Tag class="my-auto leading-3 shrink-0"
                                            :severity="isRunning(slotProps.option.uuid) ? 'success' : 'info'"
                                            :value="t(isRunning(slotProps.option.uuid) ? 'network_running' : 'network_stopped')" />
                                    </div>
                                    <div v-if="!managed" class="max-w-full overflow-hidden text-ellipsis text-gray-500">
                                        {{ slotProps.option.uuid }}
                                    </div>
                                </div>
                            </template>
                        </Select>
                        <label class="network-label mr-2 font-medium" for="dd-inst-id">{{
                            t('web.device_management.network') }}</label>
                    </IftaLabel>
                </div>

                <!-- 简化的按钮区域 - 无论屏幕大小都显示 -->
                <div v-if="!managed" class="flex gap-2 shrink-0 button-container items-center">
                    <!-- Create/Cancel button based on state -->
                    <Button v-if="!isEditingNetwork" @click="newNetwork" icon="pi pi-plus"
                        :label="screenWidth > 640 ? t('web.device_management.create_new') : undefined"
                        :class="['create-button', screenWidth <= 640 ? 'p-button-icon-only' : '']"
                        :style="screenWidth <= 640 ? 'width: 3rem !important; height: 3rem !important; font-size: 1.2rem' : ''"
                        :tooltip="screenWidth <= 640 ? t('web.device_management.create_network') : undefined"
                        tooltipOptions="{ position: 'bottom' }" severity="primary" />

                    <Button v-else @click="cancelEditNetwork" icon="pi pi-times"
                        :label="screenWidth > 640 ? t('web.device_management.cancel_edit') : undefined"
                        :class="['cancel-button', screenWidth <= 640 ? 'p-button-icon-only' : '']"
                        :style="screenWidth <= 640 ? 'width: 3rem !important; height: 3rem !important; font-size: 1.2rem' : ''"
                        :tooltip="screenWidth <= 640 ? t('web.device_management.cancel_edit') : undefined"
                        tooltipOptions="{ position: 'bottom' }" severity="secondary" />

                    <!-- More actions menu -->
                    <Menu ref="menuRef" :model="actionMenu" :popup="true" />
                    <Button v-if="!isEditingNetwork && selectedInstanceId" icon="pi pi-ellipsis-v"
                        class="p-button-rounded flex items-center justify-center" severity="help"
                        style="width: 3rem !important; height: 3rem !important; font-size: 1.2rem"
                        @click="menuRef.toggle($event)" :aria-label="t('web.device_management.more_actions')"
                        :tooltip="t('web.device_management.more_actions')" tooltipOptions="{ position: 'bottom' }" />
                </div>
            </div>
        </div>

        <section v-if="desktopView === 'devices' && !isEditingNetwork" class="desktop-workspace-network"
            :aria-label="t('desktop.current_network')">
            <span class="desktop-workspace-network-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
            <div class="desktop-workspace-network-info">
                <h2>{{ selectedInstanceId ? networkName(selectedInstanceId) : t('desktop.current_network') }}</h2>
                <code v-if="desktopNetworkAddress">{{ desktopNetworkAddress }}</code>
            </div>
            <span class="desktop-workspace-network-state" :class="{ connected: desktopCurrentNetwork?.running }">
                {{ t(desktopConnectionLabel) }}
            </span>
            <DesktopConnectionSwitch v-if="desktopConnection || (!managed && (desktopCurrentNetwork?.running || networkIsDisabled))"
                :connected="desktopConnection ? desktopConnection.available && desktopConnection.enabled : Boolean(desktopCurrentNetwork?.running)"
                :busy="desktopConnection?.busy || changingNetwork"
                :disabled="desktopConnection ? !desktopConnection.available : !canToggleDesktopNetwork"
                @toggle="desktopConnection ? emits('toggleManagedConnection') : toggleCurrentNetwork()" />
        </section>

        <!-- Main Content Area -->
        <div class="network-content">
            <!-- Network Creation Form -->
            <div v-if="!managed && (isEditingNetwork || networkIsDisabled)" class="network-creation-container">
                <div class="network-creation-header flex items-center gap-2 mb-3">
                    <i class="pi pi-plus-circle text-primary text-xl"></i>
                    <h2 class="text-xl font-medium">{{ t('web.device_management.edit_network') }}</h2>
                </div>

                <div class="w-full flex gap-2 flex-wrap justify-start mb-3">
                    <Button @click="showConfigEditDialog = true" icon="pi pi-file-edit"
                        :label="t('web.device_management.edit_as_file')" iconPos="left" severity="secondary" />
                    <Button @click="importConfig" icon="pi pi-upload" :label="t('web.device_management.import_config')"
                        iconPos="left" severity="help" />
                    <Button v-if="networkIsDisabled" @click="saveNetworkConfig" :disabled="!currentNetworkConfig"
                        icon="pi pi-save" :label="t('web.device_management.save_config')" iconPos="left"
                        severity="success" />
                </div>

                <Divider />

                <Config :cur-network="currentNetworkConfig" :config-invalid="!currentNetworkConfig"
                    @run-network="saveAndRunNewNetwork"></Config>
            </div>

            <!-- Network Status (for running networks) -->
            <div v-else-if="needShowNetworkStatus" class="network-status-container">
                <div v-if="!desktopView" class="network-status-header flex items-center gap-2 mb-3">
                    <i class="pi pi-chart-line text-primary text-xl"></i>
                    <h2 class="text-xl font-medium">{{ t('web.device_management.network_status') }}</h2>
                </div>

                <Status v-if="curNetworkInfo && curNetworkInfo.error_msg === ''" v-bind:cur-network-inst="curNetworkInfo"
                    :api="api" :managed="managed" :readonly="managed || !currentNetworkControl.editable.value"
                    :desktop-view="desktopView" :network-name="selectedInstanceId && networkName(selectedInstanceId)"
                    :requested-peer-id="requestedPeerId" @copy-address="emits('copyAddress', $event)"
                    @device-peers="desktopDevicePeers = $event">
                </Status>
                <Message v-else-if="curNetworkInfo?.error_msg" severity="error" class="mb-4">{{
                    curNetworkInfo.error_msg }}</Message>
                <Message v-else severity="info" class="mb-4">{{ t('web.device_management.loading_network_status') }}
                </Message>

                <div v-if="!managed && (desktopView !== 'devices' || !curNetworkInfo?.running)" class="text-center mt-4">
                    <Button @click="stopNetwork" :disabled="!currentNetworkControl.deletable.value" :loading="changingNetwork"
                        :label="t('web.device_management.disable_network')" severity="danger" icon="pi pi-power-off"
                        iconPos="left" />
                </div>
            </div>

            <!-- Empty State -->
            <div v-else class="empty-state flex flex-col items-center py-12" :class="{ 'desktop-empty': desktopView }">
                <i class="pi pi-sitemap text-5xl text-secondary mb-4 opacity-50"></i>
                <slot v-if="managed && !assignedNetworks?.length" name="managed-empty">
                    <div class="text-xl text-center font-medium mb-3">{{ t('controller.no_assigned_network') }}</div>
                </slot>
                <div v-else class="text-xl text-center font-medium mb-3">{{ t(managed
                    ? (desktopConnection?.enabled === false ? 'desktop.disconnected' : 'controller.applying_assignment')
                    : 'web.device_management.no_network_selected') }}
                </div>
                <p v-if="!managed" class="text-secondary text-center mb-6 max-w-md">
                    {{ t('web.device_management.select_existing_network_or_create_new') }}
                </p>
                <Button v-if="!managed" @click="newNetwork" :label="t('web.device_management.create_network')" icon="pi pi-plus"
                    iconPos="left" />
            </div>
        </div>

        <!-- Keep only the config edit dialogs -->
        <!-- <ConfigEditDialog v-if="networkIsDisabled" v-model:visible="showCreateNetworkDialog"
            :cur-network="currentNetworkConfig" :generate-config="generateConfig" :save-config="saveConfig" /> -->

        <ConfigEditDialog v-if="!managed" v-model:visible="showConfigEditDialog" :cur-network="currentNetworkConfig"
            :generate-config="generateConfig" :save-config="syncTomlConfig" />
    </div>
</template>

<style scoped>
.device-management {
    height: 100%;
    display: flex;
    flex-direction: column;
    /* hosts that mount this full-page (bilin-gui) have no padding of
       their own, so keep a safe inset here; panel chrome stays with hosts */
    padding: 0.75rem;
}

.network-content {
    flex: 1;
    overflow-y: auto;
}

.desktop-management {
    height: auto;
    padding: 0;
    gap: 18px;
}

.desktop-management .network-content {
    overflow: visible;
    min-width: 0;
}

.desktop-workspace-network {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 13px;
    min-height: 66px;
    padding: 13px 15px;
    border-radius: 10px;
    background: linear-gradient(112deg, #272727, #202020);
    color: #f5f5f5;
}

.desktop-workspace-network::after {
    content: '';
    position: absolute;
    z-index: -1;
    inset: 0;
    background: radial-gradient(circle, #686868 1px, transparent 1.3px) 1px 2px / 12px 12px;
    mask-image: linear-gradient(90deg, transparent 24%, #000);
    opacity: .48;
    pointer-events: none;
}

.desktop-workspace-network-icon {
    display: grid;
    place-items: center;
    flex: 0 0 37px;
    height: 37px;
    border: 1px solid #535353;
    border-radius: 9px;
    background: #393939;
    font-size: 16px;
}

.desktop-workspace-network-info { flex: 1 1 120px; min-width: 0; }
.desktop-workspace-network-info h2 { margin: 0; font-size: 17px; line-height: 1.25; font-weight: 700; overflow-wrap: anywhere; }
.desktop-workspace-network-info code { display: block; margin-top: 4px; color: #d7d7d7; font: 11px var(--desk-mono, monospace); }
.desktop-workspace-network-state { display: inline-flex; align-items: center; gap: 7px; font-size: 12px; white-space: nowrap; }
.desktop-workspace-network-state::before { content: ''; width: 7px; height: 7px; border-radius: 50%; background: #a0a0a0; }
.desktop-workspace-network-state.connected::before { background: #2dbe74; }

.desktop-network-toolbar, .desktop-network-select, .desktop-network-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
}

.desktop-network-toolbar {
    justify-content: space-between;
}

.desktop-network-select label {
    font-size: 13px;
    color: #575757;
}

.desktop-network-select :deep(.p-select) {
    min-width: min(215px, 54vw);
    max-width: 100%;
}

.desktop-empty {
    background: #fff;
    border: 1px solid #dedede;
    border-radius: 12px;
    min-height: 260px;
    justify-content: center;
}

.desktop-empty > i {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 12px;
    background: #f1f1f1;
    color: #333;
    opacity: 1;
    font-size: 22px;
}

/* 按钮样式 */
.button-container {
    gap: 0.5rem;
}

.create-button {
    font-weight: 600;
    min-width: 3rem;
}

/* 菜单样式定制 */
:deep(.p-menu) {
    min-width: 12rem;
    box-shadow: 0 0.5rem 1rem rgba(0, 0, 0, 0.15);
    padding: 0.25rem;
}

:deep(.p-menu .p-menuitem) {
    border-radius: 0.25rem;
}

:deep(.p-menu .p-menuitem-link) {
    padding: 0.65rem 1rem;
    font-size: 0.9rem;
}

:deep(.p-menu .p-menuitem-icon) {
    margin-right: 0.75rem;
}

:deep(.p-menu .p-menuitem.p-error .p-menuitem-text,
    .p-menu .p-menuitem.p-error .p-menuitem-icon) {
    color: var(--red-500);
}

:deep(.p-menu .p-menuitem:hover.p-error .p-menuitem-link) {
    background-color: var(--red-50);
}

/* 按钮图标样式 */
:deep(.p-button-icon-only) {
    width: 2.5rem !important;
    padding: 0.5rem !important;
}

:deep(.p-button-icon-only .p-button-icon) {
    font-size: 1rem;
}

/* 网络选择相关样式 */
.network-label {
    white-space: nowrap;
}

:deep(.network-select-container) {
    max-width: 100%;
}

/* Dark mode adaptations: surface utilities follow host theme tokens; the
   app-dark branch supplies dark fallbacks for hosts that define no tokens.
   :global must own the whole selector — mixing it with :deep in one selector
   makes the compiler drop the :deep part. */
:deep(.bg-surface-50) {
    background-color: var(--surface-ground, #f8fafc);
}

:deep(.bg-surface-0) {
    background-color: var(--surface-card, #ffffff);
}

:deep(.text-primary) {
    color: var(--primary-color, #3b82f6);
}

:deep(.text-secondary) {
    color: var(--text-color-secondary, #64748b);
}

:global(html.app-dark .device-management .bg-surface-50) {
    background-color: var(--surface-ground, #0f172a);
}

:global(html.app-dark .device-management .bg-surface-0) {
    background-color: var(--surface-card, #1e293b);
}

/* Responsive design for mobile devices */
@media (max-width: 768px) {

    /* 在小屏幕上缩短网络标签文本 */
    .network-label {
        font-size: 0.9rem;
    }
}
</style>
