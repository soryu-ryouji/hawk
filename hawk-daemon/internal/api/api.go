// Package api 是 hawk-daemon 的 HTTP 层：路由、鉴权、统一信封与 SSE。
package api

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/auth"
	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

// ListenerMode 是监听器类型：本地环回、局域网 Web、局域网直连。
type ListenerMode int

const (
	ModeLocal ListenerMode = iota
	ModeWeb
	ModePeer
)

func (m ListenerMode) Channel() library.Channel {
	switch m {
	case ModeWeb:
		return library.ChannelLanWeb
	case ModePeer:
		return library.ChannelLanPeer
	default:
		return library.ChannelLoopback
	}
}

// 错误码（客户端按 code 分支）。
const (
	CodeInvalidParam      = "INVALID_PARAM"
	CodeItemNotFound      = "ITEM_NOT_FOUND"
	CodeFolderNotFound    = "FOLDER_NOT_FOUND"
	CodeFileExists        = "FILE_EXISTS"
	CodeUnsupportedFormat = "UNSUPPORTED_FORMAT"
	CodeCategoryNotFound  = "CATEGORY_NOT_FOUND"
	CodeCategoryExists    = "CATEGORY_EXISTS"
	CodeTagNotFound       = "TAG_NOT_FOUND"
	CodeInternal          = "INTERNAL"
	CodeReadOnly          = "READ_ONLY"
	CodeUnauthorized      = "UNAUTHORIZED"
	CodeNotReady          = "NOT_READY"
	CodeLocked            = "LOCKED"
	CodeThrottled         = "THROTTLED"
	CodeLockNotFound      = "LOCK_NOT_FOUND"
	CodeOldPasswordNeeded = "OLD_PASSWORD_REQUIRED"
	CodeInvalidHost       = "INVALID_HOST"
)

type apiError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type envelope struct {
	Status string    `json:"status"`
	Data   any       `json:"data,omitempty"`
	Error  *apiError `json:"error,omitempty"`
}

// Server 承载 HTTP 路由与共享状态。
type Server struct {
	Lib     *library.Library
	Auth    *auth.Service
	Mode    ListenerMode
	WebDist string
	LAN     *LANSupervisor
}

func NewServer(lib *library.Library, svc *auth.Service, mode ListenerMode, webDist string) *Server {
	return &Server{Lib: lib, Auth: svc, Mode: mode, WebDist: webDist}
}

type contextKey int

const accessContextKey contextKey = 1

func withAccess(r *http.Request, access auth.AccessContext) *http.Request {
	return r.WithContext(context.WithValue(r.Context(), accessContextKey, access))
}

func accessFrom(r *http.Request) auth.AccessContext {
	access, _ := r.Context().Value(accessContextKey).(auth.AccessContext)
	return access
}

// Handler 返回完整中间件链。
func (s *Server) Handler() http.Handler {
	mux := s.routes()
	var handler http.Handler = mux
	handler = s.readyGate(handler)
	handler = s.authMiddleware(handler)
	handler = s.cors(handler)
	handler = s.recover(handler)
	handler = s.bodyLimit(handler)
	return handler
}

func (s *Server) routes() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /health", s.handleHealth)
	mux.HandleFunc("GET /openapi/v1.json", s.handleOpenAPI)

	mux.HandleFunc("GET /api/v1/app/info", s.handleAppInfo)
	mux.HandleFunc("GET /api/v1/app/startup", s.handleStartup)
	mux.HandleFunc("GET /api/v1/app/status", s.handleStatus)
	mux.HandleFunc("GET /api/v1/app/admin-token", s.handleAdminToken)
	mux.HandleFunc("POST /api/v1/app/session", s.handleAppSession)
	mux.HandleFunc("GET /api/v1/app/connection", s.handleConnection)
	mux.HandleFunc("GET /api/v1/app/lan", s.handleLanGet)
	mux.HandleFunc("PUT /api/v1/app/lan", s.handleLanPut)
	mux.HandleFunc("GET /api/v1/app/lan/connection-info", s.handleLanConnectionInfo)
	mux.HandleFunc("GET /api/v1/app/lan/peers", s.handleLanPeers)
	mux.HandleFunc("GET /api/v1/app/lan/clients", s.handleLanClients)
	mux.HandleFunc("POST /api/v1/app/lan/device-keys", s.handleDeviceKeyCreate)
	mux.HandleFunc("PATCH /api/v1/app/lan/device-keys/{key_id}", s.handleDeviceKeyUpdate)
	mux.HandleFunc("DELETE /api/v1/app/lan/device-keys/{key_id}", s.handleDeviceKeyDelete)
	mux.HandleFunc("POST /api/v1/app/lan/pair-requests", s.handlePairRequestCreate)
	mux.HandleFunc("GET /api/v1/app/lan/pair-requests", s.handlePairRequestList)
	mux.HandleFunc("GET /api/v1/app/lan/pair-requests/{request_id}", s.handlePairRequestStatus)
	mux.HandleFunc("POST /api/v1/app/lan/pair-requests/{request_id}/decision", s.handlePairRequestDecision)

	mux.HandleFunc("GET /api/v1/library/info", s.handleLibraryInfo)
	mux.HandleFunc("PATCH /api/v1/library/info", s.handleLibraryRename)
	mux.HandleFunc("PUT /api/v1/library/scan", s.handleLibraryScan)
	mux.HandleFunc("POST /api/v1/library/reindex", s.handleLibraryReindex)
	mux.HandleFunc("POST /api/v1/library/rescan", s.handleLibraryRescan)
	mux.HandleFunc("POST /api/v1/library/refresh_cache", s.handleRefreshCache)
	mux.HandleFunc("POST /api/v1/library/cleanup_index", s.handleCleanupIndex)

	mux.HandleFunc("GET /api/v1/folder/list", s.handleFolderList)
	mux.HandleFunc("POST /api/v1/folder/create", s.handleFolderCreate)
	mux.HandleFunc("POST /api/v1/folder/update", s.handleFolderUpdate)
	mux.HandleFunc("POST /api/v1/folder/delete", s.handleFolderDelete)
	mux.HandleFunc("POST /api/v1/folder/restore", s.handleFolderRestore)

	mux.HandleFunc("POST /api/v1/item/list", s.handleItemList)
	mux.HandleFunc("POST /api/v1/item/skeleton", s.handleItemSkeleton)
	mux.HandleFunc("POST /api/v1/item/duplicates", s.handleItemDuplicates)
	mux.HandleFunc("POST /api/v1/item/aggregate", s.handleItemAggregate)
	mux.HandleFunc("GET /api/v1/item/detail", s.handleItemDetail)
	mux.HandleFunc("GET /api/v1/item/count", s.handleItemCount)
	mux.HandleFunc("POST /api/v1/item/add", s.handleItemAdd)
	mux.HandleFunc("POST /api/v1/item/upload", s.handleItemUpload)
	mux.HandleFunc("POST /api/v1/item/update", s.handleItemUpdate)
	mux.HandleFunc("POST /api/v1/item/batch_update", s.handleItemBatchUpdate)
	mux.HandleFunc("POST /api/v1/item/delete", s.handleItemDelete)
	mux.HandleFunc("POST /api/v1/item/restore", s.handleItemRestore)
	mux.HandleFunc("GET /api/v1/item/thumbnail", s.handleItemThumbnail)
	mux.HandleFunc("GET /api/v1/item/file", s.handleItemFile)
	mux.HandleFunc("POST /api/v1/item/refresh_thumbnail", s.handleRefreshThumbnail)
	mux.HandleFunc("POST /api/v1/item/replace", s.handleItemReplace)

	mux.HandleFunc("GET /api/v1/category/list", s.handleCategoryList)
	mux.HandleFunc("POST /api/v1/category/create", s.handleCategoryCreate)
	mux.HandleFunc("POST /api/v1/category/update", s.handleCategoryUpdate)
	mux.HandleFunc("POST /api/v1/category/delete", s.handleCategoryDelete)
	mux.HandleFunc("GET /api/v1/tag/list", s.handleTagList)
	mux.HandleFunc("POST /api/v1/tag/create", s.handleTagCreate)
	mux.HandleFunc("POST /api/v1/tag/update", s.handleTagUpdate)
	mux.HandleFunc("POST /api/v1/tag/delete", s.handleTagDelete)

	mux.HandleFunc("GET /api/v1/view/preferences", s.handleViewPreferences)
	mux.HandleFunc("PUT /api/v1/view/preference", s.handleViewPreferencePut)
	mux.HandleFunc("DELETE /api/v1/view/preference", s.handleViewPreferenceDelete)

	mux.HandleFunc("GET /api/v1/global_filter/list", s.handleGlobalFilterList)
	mux.HandleFunc("PUT /api/v1/global_filter", s.handleGlobalFilterPut)

	mux.HandleFunc("GET /api/v1/lock/list", s.handleLockList)
	mux.HandleFunc("POST /api/v1/lock/set", s.handleLockSet)
	mux.HandleFunc("POST /api/v1/lock/remove", s.handleLockRemove)
	mux.HandleFunc("POST /api/v1/lock/unlock", s.handleLockUnlock)

	mux.HandleFunc("POST /api/v1/trash/clear", s.handleTrashClear)
	mux.HandleFunc("GET /api/v1/events", s.handleEvents)

	mux.HandleFunc("/", s.handleStatic)
	return mux
}

func (s *Server) recover(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			if value := recover(); value != nil {
				writeError(w, http.StatusInternalServerError, CodeInternal, fmt.Sprintf("internal error: %v", value))
			}
		}()
		next.ServeHTTP(w, r)
	})
}

func (s *Server) bodyLimit(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Body != nil {
			r.Body = http.MaxBytesReader(w, r.Body, 256*1024*1024)
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) cors(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		isDiscovery := r.URL.Path == "/api/v1/app/admin-token"
		if r.Method == http.MethodOptions && !isDiscovery {
			w.Header().Set("Access-Control-Allow-Origin", "*")
			w.Header().Set("Access-Control-Allow-Methods", "*")
			w.Header().Set("Access-Control-Allow-Headers", "*")
			w.WriteHeader(http.StatusNoContent)
			return
		}
		if !isDiscovery {
			w.Header().Set("Access-Control-Allow-Origin", "*")
			w.Header().Set("Access-Control-Allow-Methods", "*")
			w.Header().Set("Access-Control-Allow-Headers", "*")
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) readyGate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := r.URL.Path
		if strings.HasPrefix(path, "/api/") && path != "/api/v1/app/startup" && !s.Lib.Startup.IsReady() {
			writeError(w, http.StatusServiceUnavailable, CodeNotReady, "initial index is still building")
			return
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) authMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := r.URL.Path
		if !strings.HasPrefix(path, "/api") {
			next.ServeHTTP(w, r)
			return
		}
		if path == "/api/v1/app/admin-token" && r.Method == http.MethodGet {
			next.ServeHTTP(w, r)
			return
		}
		if path == "/api/v1/app/session" && r.Method == http.MethodPost && s.Mode == ModeWeb {
			next.ServeHTTP(w, r)
			return
		}
		if s.isPairingPath(path, r.Method) {
			next.ServeHTTP(w, r)
			return
		}

		access, ok := s.resolveAccess(r)
		if !ok {
			writeError(w, http.StatusUnauthorized, CodeUnauthorized, "missing or invalid token")
			return
		}
		if err := access.Require(requiredCapability(r.Method, path)); err != nil {
			writeError(w, http.StatusForbidden, CodeReadOnly, err.Error())
			return
		}
		if s.Mode == ModeWeb && !cookieWriteAllowed(r) {
			writeError(w, http.StatusForbidden, CodeReadOnly, "Cookie 写请求缺少合法 Origin 或 X-Hawk-Client")
			return
		}
		if s.Mode == ModePeer && access.KeyID != "" && s.LAN != nil {
			s.LAN.RecordPeerActivity(access.KeyID)
		}
		next.ServeHTTP(w, withAccess(r, access))
	})
}

// cookieWriteAllowed：只用 Cookie 认证的写请求必须带同源 Origin 与自定义头，
// 防止同网段网页用表单/图片发起 CSRF 写请求。
func cookieWriteAllowed(r *http.Request) bool {
	if r.Method == http.MethodGet || r.Method == http.MethodHead || r.Method == http.MethodOptions {
		return true
	}
	if r.Header.Get("Authorization") != "" {
		return true
	}
	if _, err := r.Cookie("hawk_session"); err != nil {
		return true
	}
	if r.Header.Get("X-Hawk-Client") != "web" {
		return false
	}
	origin := r.Header.Get("Origin")
	host := r.Host
	return origin == "http://"+host || origin == "https://"+host
}

func (s *Server) isPairingPath(path, method string) bool {
	if s.Mode != ModePeer {
		return false
	}
	switch {
	case path == "/api/v1/app/lan/connection-info" && method == http.MethodGet:
		return true
	case path == "/api/v1/app/lan/pair-requests" && method == http.MethodPost:
		return true
	case strings.HasPrefix(path, "/api/v1/app/lan/pair-requests/") && method == http.MethodGet:
		rest := strings.TrimPrefix(path, "/api/v1/app/lan/pair-requests/")
		return rest != "" && !strings.Contains(rest, "/")
	}
	return false
}

func (s *Server) resolveAccess(r *http.Request) (auth.AccessContext, bool) {
	channel := s.Mode.Channel()
	if header := r.Header.Get("Authorization"); strings.HasPrefix(header, "Bearer ") {
		if access, ok := s.Auth.ResolveCredential(strings.TrimPrefix(header, "Bearer "), channel); ok {
			return access, true
		}
	}
	if s.Mode == ModeWeb {
		if cookie, err := r.Cookie("hawk_session"); err == nil {
			if keyID, ok := s.Auth.ResolveSession(cookie.Value); ok {
				return s.Auth.SessionAccess(keyID, library.ChannelLanWeb)
			}
		}
	}
	// 图片/SSE 直链无法设头，保留 ?token= 兼容。
	if token := r.URL.Query().Get("token"); token != "" {
		if access, ok := s.Auth.ResolveCredential(token, channel); ok {
			return access, true
		}
	}
	return auth.AccessContext{}, false
}

// requiredCapability 是路由能力表；未声明的端点默认拒绝。
func requiredCapability(method, path string) library.Capability {
	if path == "/api/v1/app/connection" {
		return library.CapImport
	}
	if path == "/api/v1/library/info" {
		if method == http.MethodGet {
			return library.CapRead
		}
		return library.CapManage
	}
	if managePaths[path] || strings.HasPrefix(path, "/api/v1/app/lan/device-keys/") {
		return library.CapManage
	}
	if method == http.MethodGet && readPaths[path] {
		return library.CapRead
	}
	if method == http.MethodPost && postReadPaths[path] {
		return library.CapRead
	}
	if method == http.MethodPost && importPaths[path] {
		return library.CapImport
	}
	if editPaths[path] {
		return library.CapEdit
	}
	return library.CapManage
}

var readPaths = map[string]bool{
	"/api/v1/app/info":                true,
	"/api/v1/app/lan/connection-info": true,
	"/api/v1/app/startup":             true,
	"/api/v1/app/status":              true,
	"/api/v1/category/list":           true,
	"/api/v1/events":                  true,
	"/api/v1/folder/list":             true,
	"/api/v1/global_filter/list":      true,
	"/api/v1/item/count":              true,
	"/api/v1/item/detail":             true,
	"/api/v1/item/file":               true,
	"/api/v1/item/thumbnail":          true,
	"/api/v1/lock/list":               true,
	"/api/v1/tag/list":                true,
	"/api/v1/view/preferences":        true,
	"/openapi/v1.json":                true,
}

var postReadPaths = map[string]bool{
	"/api/v1/app/session":    true,
	"/api/v1/item/aggregate": true,
	"/api/v1/item/list":      true,
	"/api/v1/item/skeleton":  true,
	"/api/v1/lock/unlock":    true,
}

var importPaths = map[string]bool{
	"/api/v1/folder/create": true,
	"/api/v1/item/add":      true,
	"/api/v1/item/upload":   true,
}

var editPaths = map[string]bool{
	"/api/v1/category/create":        true,
	"/api/v1/category/delete":        true,
	"/api/v1/category/update":        true,
	"/api/v1/folder/delete":          true,
	"/api/v1/folder/restore":         true,
	"/api/v1/folder/update":          true,
	"/api/v1/global_filter":          true,
	"/api/v1/item/batch_update":      true,
	"/api/v1/item/delete":            true,
	"/api/v1/item/refresh_thumbnail": true,
	"/api/v1/item/replace":           true,
	"/api/v1/item/restore":           true,
	"/api/v1/item/update":            true,
	"/api/v1/tag/create":             true,
	"/api/v1/tag/delete":             true,
	"/api/v1/tag/update":             true,
	"/api/v1/trash/clear":            true,
	"/api/v1/view/preference":        true,
}

var managePaths = map[string]bool{
	"/api/v1/app/connection":        true,
	"/api/v1/app/lan":               true,
	"/api/v1/app/lan/clients":       true,
	"/api/v1/app/lan/device-keys":   true,
	"/api/v1/app/lan/pair-requests": true,
	"/api/v1/app/lan/peers":         true,
	"/api/v1/item/duplicates":       true,
	"/api/v1/library/cleanup_index": true,
	"/api/v1/library/refresh_cache": true,
	"/api/v1/library/reindex":       true,
	"/api/v1/library/rescan":        true,
	"/api/v1/library/scan":          true,
	"/api/v1/lock/remove":           true,
	"/api/v1/lock/set":              true,
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

func writeOK(w http.ResponseWriter, data any) {
	writeJSON(w, http.StatusOK, envelope{Status: "success", Data: data})
}

func writeSuccess(w http.ResponseWriter) {
	writeJSON(w, http.StatusOK, envelope{Status: "success"})
}

func writeError(w http.ResponseWriter, status int, code, message string) {
	writeJSON(w, status, envelope{Status: "error", Error: &apiError{Code: code, Message: message}})
}

func decodeJSON(r *http.Request, dest any) error {
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(dest); err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			return fmt.Errorf("请求体超过 256MB")
		}
		return fmt.Errorf("请求体解析失败: %w", err)
	}
	return nil
}

func requireAdmin(w http.ResponseWriter, r *http.Request) bool {
	if !accessFrom(r).IsAdmin() {
		writeError(w, http.StatusForbidden, CodeReadOnly, "仅本机管理员可执行此操作")
		return false
	}
	return true
}

func nowMillis() int64 { return time.Now().UnixMilli() }
