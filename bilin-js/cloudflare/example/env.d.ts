interface Env {
  BILIN_CORE: DurableObjectNamespace<
    import("./worker").BilinCoreObject
  >;
  BILIN_NETWORK_NAME: string;
  BILIN_NETWORK_SECRET: string;
  BILIN_ENABLE_ENCRYPTION?: string;
  BILIN_OBJECT_NAME?: string;
}
