export type BilinState =
  | "created"
  | "starting"
  | "running"
  | "stopping"
  | "stopped";

export interface BilinNetworkConfig {
  networkName: string;
  networkSecret: string;
  instanceName?: string;
  encryption?: boolean;
}

export interface BilinEvent {
  kind: string;
  message: string;
}

export interface BilinStatus {
  state: BilinState;
  connections: number;
}

export interface BilinOperationOptions {
  /** Timeout in milliseconds. Omit to wait indefinitely. */
  timeout?: number;
}

export interface BilinIpv4SocketAddress {
  ipv4: string;
  port: number;
}

export interface BilinTcpReadResult {
  data: Uint8Array;
  eof: boolean;
}

export interface BilinTcpStream {
  readonly localAddress: BilinIpv4SocketAddress;
  readonly peerAddress: BilinIpv4SocketAddress;
  read(maxLength?: number): Promise<BilinTcpReadResult>;
  write(data: Uint8Array): Promise<number>;
  shutdownWrite(): Promise<void>;
  close(): Promise<void>;
}

export interface BilinTcpListener {
  readonly localAddress: BilinIpv4SocketAddress;
  accept(options?: BilinOperationOptions): Promise<BilinTcpStream>;
  close(): Promise<void>;
}

export interface BilinInstance {
  connectTcp(
    address: string,
    options?: BilinOperationOptions,
  ): Promise<BilinTcpStream>;
  listenTcp(
    port: number,
    options?: BilinOperationOptions,
  ): Promise<BilinTcpListener>;
  status(): Promise<BilinStatus>;
  close(): Promise<void>;
}
