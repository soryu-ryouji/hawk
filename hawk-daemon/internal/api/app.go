package api

import (
	"net"
	"net/http"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

const daemonVersion = "0.1.0"

type lanInfo struct {
	Active     bool   `json:"active"`
	Port       *int   `json:"port,omitempty"`
	Error      string `json:"error,omitempty"`
	PeerActive bool   `json:"peer_active"`
	PeerPort   *int   `json:"peer_port,omitempty"`
	PeerError  string `json:"peer_error,omitempty"`
}

type appInfo struct {
	Version    string  `json:"version"`
	Platform   string  `json:"platform"`
	ExecPath   string  `json:"exec_path"`
	Role       string  `json:"role"`
	Channel    string  `json:"channel"`
	HasLibrary bool    `json:"has_library"`
	Lan        lanInfo `json:"lan"`
}

func (s *Server) handleAppInfo(w http.ResponseWriter, r *http.Request) {
	access := accessFrom(r)
	execPath, _ := os.Executable()
	info := appInfo{
		Version:    daemonVersion,
		Platform:   platformName(),
		ExecPath:   execPath,
		Role:       string(access.Role),
		Channel:    string(access.Channel),
		HasLibrary: true,
		Lan:        s.lanInfo(),
	}
	writeOK(w, info)
}

func (s *Server) lanInfo() lanInfo {
	if s.LAN == nil {
		return lanInfo{Active: false, PeerActive: false}
	}
	status := s.LAN.Status()
	info := lanInfo{
		Active:     status.Web.Active,
		PeerActive: status.Peer.Active,
	}
	if status.Web.Active {
		port := status.Web.Port
		info.Port = &port
	}
	if status.Web.Error != "" {
		info.Error = status.Web.Error
	}
	if status.Peer.Active {
		port := status.Peer.Port
		info.PeerPort = &port
	}
	if status.Peer.Error != "" {
		info.PeerError = status.Peer.Error
	}
	return info
}

func platformName() string {
	switch runtime.GOOS {
	case "windows":
		return "windows"
	case "darwin":
		return "macos"
	default:
		return "linux"
	}
}

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	if !s.Lib.Startup.IsReady() {
		w.WriteHeader(http.StatusServiceUnavailable)
		return
	}
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte("ok"))
}

func (s *Server) handleStartup(w http.ResponseWriter, r *http.Request) {
	status, phase, message, processed, total := s.Lib.Startup.Snapshot()
	payload := map[string]any{"status": status}
	if status == "starting" {
		payload["phase"] = phase
		payload["processed"] = processed
		payload["total"] = total
	}
	if status == "error" {
		payload["message"] = message
	}
	writeOK(w, payload)
}

type taskBacklog struct {
	Pending int `json:"pending"`
	Active  int `json:"active"`
}

type indexBacklog struct {
	Pending   int    `json:"pending"`
	Active    int    `json:"active"`
	Phase     string `json:"phase,omitempty"`
	Processed int    `json:"processed,omitempty"`
	Total     int    `json:"total,omitempty"`
}

type scanStatsInfo struct {
	StartedUnixMS int64 `json:"started_unix_ms"`
	DurationMS    int64 `json:"duration_ms"`
	Files         int   `json:"files"`
	DirtyDirs     int   `json:"dirty_dirs"`
	Applied       int   `json:"applied"`
}

type taskStatus struct {
	Thumbnail     taskBacklog    `json:"thumbnail"`
	Index         indexBacklog   `json:"index"`
	LastScan      *scanStatsInfo `json:"last_scan"`
	QueueOverflow bool           `json:"queue_overflow"`
	SSELagged     uint64         `json:"sse_lagged"`
	ConfigError   string         `json:"config_error,omitempty"`
}

func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	thumb := s.Lib.ThumbnailStats()
	status := taskStatus{
		Thumbnail:     taskBacklog{Pending: thumb.Pending, Active: thumb.Active},
		Index:         indexBacklog{},
		QueueOverflow: s.Lib.QueueOverflowed(),
		SSELagged:     s.Lib.Bus.Lagged(),
		ConfigError:   s.Lib.Config.Error(),
	}
	if stats := s.Lib.LastScanStats(); stats != nil {
		status.LastScan = &scanStatsInfo{
			StartedUnixMS: stats.StartedUnixMS,
			DurationMS:    stats.DurationMS,
			Files:         stats.Files,
			DirtyDirs:     stats.DirtyDirs,
			Applied:       stats.Applied,
		}
	}
	writeOK(w, status)
}

func (s *Server) handleAdminToken(w http.ResponseWriter, r *http.Request) {
	if !isLoopbackHost(r.Host) {
		writeError(w, http.StatusForbidden, CodeInvalidHost, "admin-token 仅接受环回地址访问")
		return
	}
	token := s.Auth.CreateIntegrationToken()
	writeOK(w, token)
}

func isLoopbackHost(host string) bool {
	host = strings.TrimSpace(host)
	if h, _, err := net.SplitHostPort(host); err == nil {
		host = h
	}
	host = strings.Trim(host, "[]")
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (s *Server) handleAppSession(w http.ResponseWriter, r *http.Request) {
	var body struct {
		AccessKey string `json:"access_key"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	key, ok := s.Auth.Keys.Resolve(s.Auth.Identity.LibraryID, strings.TrimSpace(body.AccessKey))
	if !ok {
		writeError(w, http.StatusUnauthorized, CodeUnauthorized, "访问密钥无效")
		return
	}
	token := s.Auth.CreateWebSession(key)
	http.SetCookie(w, &http.Cookie{
		Name:     "hawk_session",
		Value:    token,
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteStrictMode,
		MaxAge:   int((30 * 24 * time.Hour).Seconds()),
	})
	writeOK(w, map[string]any{
		"role":       key.Role,
		"channel":    library.ChannelLanWeb,
		"expires_in": int64((30 * 24 * time.Hour).Seconds()),
	})
}

func (s *Server) handleConnection(w http.ResponseWriter, r *http.Request) {
	writeOK(w, map[string]any{
		"mode": "local",
		"role": library.RoleAdmin,
	})
}

// currentAccess 返回请求上下文中的授权信息（测试辅助）。
func currentAccess(r *http.Request) (string, library.Role) {
	access := accessFrom(r)
	return string(access.Channel), access.Role
}
