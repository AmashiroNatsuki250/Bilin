package engine

import (
	"context"
	"fmt"

	"github.com/AmashiroNatsuki250/Bilin/bilin-go/internal/coreabi"
)

func (host *Host) SetLogLevel(ctx context.Context, level coreabi.LogLevel) error {
	if host == nil {
		return fmt.Errorf("set log level with nil Bilin engine host")
	}
	if ctx == nil {
		return fmt.Errorf("set Bilin log level with nil context")
	}
	host.mu.Lock()
	defer host.mu.Unlock()
	if host.closed {
		return fmt.Errorf("set log level with closed Bilin engine host")
	}
	host.guestMu.Lock()
	defer host.guestMu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	return coreabi.SetLogLevel(ctx, host.module, level)
}

func (host *Host) LogLevel(ctx context.Context) (coreabi.LogLevel, error) {
	if host == nil {
		return 0, fmt.Errorf("get log level with nil Bilin engine host")
	}
	if ctx == nil {
		return 0, fmt.Errorf("get Bilin log level with nil context")
	}
	host.mu.Lock()
	defer host.mu.Unlock()
	if host.closed {
		return 0, fmt.Errorf("get log level with closed Bilin engine host")
	}
	host.guestMu.Lock()
	defer host.guestMu.Unlock()
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	return coreabi.GetLogLevel(ctx, host.module)
}
