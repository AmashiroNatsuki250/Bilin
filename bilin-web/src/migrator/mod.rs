use sea_orm_migration::prelude::*;

pub(crate) const INITIAL_ADMIN_PASSWORD_HASH: &str =
    "$argon2i$v=19$m=16,t=2,p=1$bW5idXl0cmY$61n+JxL4r3dwLPAEDlDdtg";

mod m20241029_000001_init;
mod m20260403_000002_scope_network_config_unique;
mod m20260421_000003_add_network_config_source;
mod m20260514_000004_rename_web_config_source;
mod m20260619_000005_managed_config_revisions;
mod m20260920_000007_device_registry;
mod m20260920_000009_central_network_intent;

mod m20261001_000010_member_ip_allocation;
mod m20261126_000011_controller_devices;
mod m20261126_000012_forwarding_nodes;
mod m20261126_000013_device_codes;
mod m20261126_000014_controller_upstream_subnet;
mod m20261126_000015_account_connection_policy;
mod m20261126_000016_protocol_override;
mod m20261127_000017_remove_faketcp;
mod m20261127_000018_device_login;
mod m20261127_000019_mobile_account_authorization;
mod m20261127_000020_controller_device_name;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20241029_000001_init::Migration),
            Box::new(m20260403_000002_scope_network_config_unique::Migration),
            Box::new(m20260421_000003_add_network_config_source::Migration),
            Box::new(m20260514_000004_rename_web_config_source::Migration),
            Box::new(m20260619_000005_managed_config_revisions::Migration),
            Box::new(m20260920_000007_device_registry::Migration),
            Box::new(m20260920_000009_central_network_intent::Migration),
            Box::new(m20261001_000010_member_ip_allocation::Migration),
            Box::new(m20261126_000011_controller_devices::Migration),
            Box::new(m20261126_000012_forwarding_nodes::Migration),
            Box::new(m20261126_000013_device_codes::Migration),
            Box::new(m20261126_000014_controller_upstream_subnet::Migration),
            Box::new(m20261126_000015_account_connection_policy::Migration),
            Box::new(m20261126_000016_protocol_override::Migration),
            Box::new(m20261127_000017_remove_faketcp::Migration),
            Box::new(m20261127_000018_device_login::Migration),
            Box::new(m20261127_000019_mobile_account_authorization::Migration),
            Box::new(m20261127_000020_controller_device_name::Migration),
        ]
    }
}
