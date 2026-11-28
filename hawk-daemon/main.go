// hawk-daemon：纯局域网素材库后端。
package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/api"
	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/auth"
	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

const version = "0.1.0"

func main() {
	log.SetFlags(log.LstdFlags | log.Lmsgprefix)
	log.SetPrefix("hawk-daemon ")

	args := os.Args[1:]
	if len(args) > 0 && args[0] == "openapi" {
		_, _ = os.Stdout.Write(api.OpenAPISpec())
		return
	}
	if len(args) == 0 || args[0] != "local" {
		fmt.Fprintln(os.Stderr, "用法: hawk-daemon local <素材库路径> [--port 27371] [--web-dist <dir>] [--cache-parent <dir>] [--identity-dir <dir>]")
		os.Exit(2)
	}

	fs := flag.NewFlagSet("local", flag.ExitOnError)
	port := fs.Int("port", envInt("HAWK_PORT", 27371), "本地 API 端口（严格绑定，0 = 系统分配）")
	webDist := fs.String("web-dist", os.Getenv("HAWK_WEB_DIST"), "局域网 Web 前端产物目录")
	cacheParent := fs.String("cache-parent", os.Getenv("HAWK_CACHE_PARENT"), "缩略图缓存父目录")
	identityDir := fs.String("identity-dir", os.Getenv("HAWK_IDENTITY_DIR"), "访问密钥/身份目录")
	// Go flag 在第一个非 flag 参数处停止；路径固定是第一个位置参数，
	// 先把它摘出来，再解析其余 flag（兼容 `local <path> --port ...` 的既有调用）。
	rest := args[1:]
	root := ""
	filtered := make([]string, 0, len(rest))
	for _, arg := range rest {
		if root == "" && !strings.HasPrefix(arg, "-") {
			root = arg
			continue
		}
		filtered = append(filtered, arg)
	}
	if root == "" {
		fmt.Fprintln(os.Stderr, "缺少素材库路径")
		os.Exit(2)
	}
	_ = fs.Parse(filtered)

	identity := *identityDir
	if identity == "" {
		identity = defaultIdentityDir()
	}

	adminToken := os.Getenv("HAWK_ADMIN_TOKEN")
	if adminToken == "" {
		adminToken = randomToken()
		fmt.Printf("HAWK_ADMIN_TOKEN=%s\n", adminToken)
	}

	lib, err := library.OpenLibrary(root, *cacheParent)
	if err != nil {
		log.Fatalf("打开素材库失败: %v", err)
	}
	lib.Start()
	defer lib.Close()

	identityInfo := auth.BuildIdentity(hostname(), lib.Paths.Root, lib.LibraryKey(), lib.LibraryName())
	svc, err := auth.NewService(adminToken, identity, identityInfo)
	if err != nil {
		log.Fatalf("初始化访问密钥失败: %v", err)
	}

	server := api.NewServer(lib, svc, api.ModeLocal, *webDist)
	lan := api.NewLANSupervisor(server)
	lib.OnWebChanged = lan.Rebind
	lan.Start()
	defer lan.Close()

	address := fmt.Sprintf("127.0.0.1:%d", *port)
	listener, err := net.Listen("tcp", address)
	if err != nil {
		fmt.Fprintf(os.Stderr, "PORT_IN_USE %d\n", *port)
		os.Exit(4)
	}
	actualPort := listener.Addr().(*net.TCPAddr).Port
	httpServer := &http.Server{
		Handler:           server.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	log.Printf("v%s 就绪 http://127.0.0.1:%d 库=%s", version, actualPort, lib.Paths.Root)

	go func() {
		if serveErr := httpServer.Serve(listener); serveErr != nil && !errors.Is(serveErr, http.ErrServerClosed) {
			log.Fatalf("HTTP 服务退出: %v", serveErr)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
	<-stop
	log.Printf("正在退出")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = httpServer.Shutdown(ctx)
}

func defaultIdentityDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(".hawk", "remote")
	}
	// 与旧版应用级身份目录保持一致：~/.config/hawk/remote
	return filepath.Join(home, ".config", "hawk", "remote")
}

func hostname() string {
	name, err := os.Hostname()
	if err != nil || name == "" {
		return "hawk"
	}
	return name
}

func randomToken() string {
	buf := make([]byte, 32)
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}

func envInt(name string, fallback int) int {
	raw := os.Getenv(name)
	if raw == "" {
		return fallback
	}
	var value int
	if _, err := fmt.Sscanf(raw, "%d", &value); err != nil {
		return fallback
	}
	return value
}
