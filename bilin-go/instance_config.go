package host

import internalhost "github.com/AmashiroNatsuki250/Bilin/bilin-go/internal/host"

// P2PPolicy controls when an instance attempts peer-to-peer connections.
type P2PPolicy = internalhost.P2PPolicy

// HolePunchingPolicy selects the hole-punching methods available to an instance.
type HolePunchingPolicy = internalhost.HolePunchingPolicy

// PortForwardProtocol identifies the transport used by a port-forward rule.
type PortForwardProtocol = internalhost.PortForwardProtocol

const (
	PortForwardTCP PortForwardProtocol = internalhost.PortForwardTCP
	PortForwardUDP PortForwardProtocol = internalhost.PortForwardUDP
)

// PortForwardConfig exposes a host socket through the Bilin data plane.
type PortForwardConfig = internalhost.PortForwardConfig

// InstanceConfig is an immutable Bilin instance configuration.
type InstanceConfig = internalhost.InstanceConfig

// InstanceConfigBuilder builds a validated InstanceConfig.
type InstanceConfigBuilder = internalhost.InstanceConfigBuilder

// NewInstanceConfigBuilder starts a configuration for networkName.
func NewInstanceConfigBuilder(networkName string) *InstanceConfigBuilder {
	return internalhost.NewInstanceConfigBuilder(networkName)
}
