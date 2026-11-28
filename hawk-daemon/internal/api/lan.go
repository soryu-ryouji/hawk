package api

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/auth"
	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

const (
	discoveryPort = 27374
	pairTTL       = 90 * time.Second
	maxPending    = 8
	onlineWindow  = 30 * time.Second
	pairProtocol  = 3
)

type listenerStatus struct {
	Active bool
	Port   int
	Error  string
}

type lanStatuses struct {
	Web  listenerStatus
	Peer listenerStatus
}

// LANSupervisor 负责 Web/Peer 双监听、UDP 发现与配对请求。
type LANSupervisor struct {
	base *Server

	mu         sync.Mutex
	webServer  *http.Server
	peerServer *http.Server
	status     lanStatuses

	pairMu   sync.Mutex
	pairs    map[string]*pairTicket
	peerSeen map[string]int64

	udpConn *net.UDPConn
	stopCh  chan struct{}
}

type pairTicket struct {
	ID          string
	ClientName  string
	Role        library.Role
	CreatedAt   time.Time
	ExpiresAt   time.Time
	State       string // pending / approved / denied
	AccessKey   string
	GrantedRole library.Role
	Reverse     *reversePeerDTO
}

type reversePeerDTO struct {
	PeerID          string       `json:"peer_id"`
	Name            string       `json:"name"`
	LibraryName     string       `json:"library_name"`
	URLs            []string     `json:"urls"`
	PeerKey         string       `json:"peer_key"`
	Role            library.Role `json:"role"`
	TargetLibraryID string       `json:"target_library_id,omitempty"`
}

func NewLANSupervisor(base *Server) *LANSupervisor {
	sup := &LANSupervisor{
		base:     base,
		pairs:    map[string]*pairTicket{},
		peerSeen: map[string]int64{},
		stopCh:   make(chan struct{}),
	}
	base.LAN = sup
	return sup
}

func (s *LANSupervisor) Start() {
	s.Rebind()
	go s.discoveryResponder()
}

func (s *LANSupervisor) Close() {
	close(s.stopCh)
	s.mu.Lock()
	web, peer := s.webServer, s.peerServer
	s.mu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if web != nil {
		_ = web.Shutdown(ctx)
	}
	if peer != nil {
		_ = peer.Shutdown(ctx)
	}
	if s.udpConn != nil {
		_ = s.udpConn.Close()
	}
}

func (s *LANSupervisor) Status() lanStatuses {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status
}

func (s *LANSupervisor) RecordPeerActivity(keyID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.peerSeen[keyID] = time.Now().UnixMilli()
}

func (s *LANSupervisor) Clients() []lanClientDTO {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now().UnixMilli()
	keys := s.base.Auth.Keys.List(s.base.Auth.Identity.LibraryID)
	out := make([]lanClientDTO, 0)
	for keyID, lastSeen := range s.peerSeen {
		var key *auth.KeyEntry
		for i := range keys {
			if keys[i].ID == keyID {
				key = &keys[i]
				break
			}
		}
		if key == nil {
			continue
		}
		client := lanClientDTO{
			KeyID:      key.ID,
			Label:      key.Label,
			LastSeenMS: lastSeen,
			Online:     now-lastSeen <= int64(onlineWindow/time.Millisecond),
			Role:       key.Role,
		}
		if key.PeerID != "" {
			client.PeerID = &key.PeerID
		}
		if key.LibraryName != "" {
			client.LibraryName = &key.LibraryName
		}
		if key.URL != "" {
			client.URL = &key.URL
		}
		out = append(out, client)
	}
	return out
}

// Rebind 按当前配置收敛两个监听器。
func (s *LANSupervisor) Rebind() {
	config := s.base.Lib.Config.Current()
	webWanted := config.Web.Enabled && len(s.base.Auth.Keys.List(s.base.Auth.Identity.LibraryID)) > 0
	webPort := config.Web.Port
	peerPort := config.Web.PeerPort

	s.mu.Lock()
	defer s.mu.Unlock()

	webChanged := (s.webServer != nil) != webWanted ||
		(webWanted && s.status.Web.Port != webPort) ||
		(s.status.Web.Active && !webWanted)
	peerChanged := s.peerServer == nil || s.status.Peer.Port != peerPort
	if !webChanged && !peerChanged {
		return
	}

	if webChanged {
		stopServerLocked(&s.webServer)
		s.status.Web = listenerStatus{}
		if webWanted {
			server, port, err := s.bind(webPort, ModeWeb, peerPort)
			if err != nil {
				s.status.Web.Error = err.Error()
			} else {
				s.webServer = server
				s.status.Web = listenerStatus{Active: true, Port: port}
			}
		}
	}
	if peerChanged {
		stopServerLocked(&s.peerServer)
		s.status.Peer = listenerStatus{}
		server, port, err := s.bind(peerPort, ModePeer, webPort)
		if err != nil {
			s.status.Peer.Error = err.Error()
		} else {
			s.peerServer = server
			s.status.Peer = listenerStatus{Active: true, Port: port}
		}
	}
}

func stopServerLocked(server **http.Server) {
	if *server == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	_ = (*server).Shutdown(ctx)
	*server = nil
}

func (s *LANSupervisor) bind(port int, mode ListenerMode, otherPort int) (*http.Server, int, error) {
	actualPort := port
	var listener net.Listener
	var err error
	for attempt := 0; attempt <= 32; attempt++ {
		if actualPort == otherPort {
			actualPort++
			continue
		}
		listener, err = net.Listen("tcp", fmt.Sprintf("0.0.0.0:%d", actualPort))
		if err == nil {
			break
		}
		actualPort++
	}
	if listener == nil {
		return nil, 0, fmt.Errorf("端口 %d 绑定失败: %v", port, err)
	}
	handler := NewServer(s.base.Lib, s.base.Auth, mode, s.base.WebDist)
	handler.LAN = s
	server := &http.Server{Handler: handler.Handler()}
	go func() {
		if serveErr := server.Serve(listener); serveErr != nil && serveErr != http.ErrServerClosed {
			fmt.Printf("LAN %s 监听退出: %v\n", modeName(mode), serveErr)
		}
	}()
	return server, actualPort, nil
}

func modeName(mode ListenerMode) string {
	if mode == ModeWeb {
		return "web"
	}
	return "peer"
}

// ---------- UDP 发现 ----------

type discoveryMessage struct {
	Hawk        int    `json:"hawk"`
	Op          string `json:"op"`
	ID          string `json:"id"`
	Name        string `json:"name"`
	LibraryName string `json:"library_name"`
	PeerPort    int    `json:"peer_port"`
	Writable    bool   `json:"writable"`
	TargetLibID string `json:"target_library_id"`
}

func (s *LANSupervisor) discoveryResponder() {
	conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero, Port: discoveryPort})
	if err != nil {
		fmt.Printf("UDP 发现监听失败（端口 %d）: %v\n", discoveryPort, err)
		return
	}
	s.mu.Lock()
	s.udpConn = conn
	s.mu.Unlock()
	buf := make([]byte, 4096)
	for {
		_ = conn.SetReadDeadline(time.Now().Add(time.Second))
		n, addr, err := conn.ReadFromUDP(buf)
		if err != nil {
			select {
			case <-s.stopCh:
				return
			default:
				continue
			}
		}
		var message discoveryMessage
		if err := json.Unmarshal(buf[:n], &message); err != nil || message.Op != "probe" {
			continue
		}
		reply := s.announce()
		data, err := json.Marshal(reply)
		if err != nil {
			continue
		}
		_, _ = conn.WriteToUDP(data, addr)
	}
}

func (s *LANSupervisor) announce() discoveryMessage {
	status := s.Status()
	identity := s.base.Auth.Identity
	writable := false
	for _, key := range s.base.Auth.Keys.List(identity.LibraryID) {
		if key.Role == library.RoleEditor {
			writable = true
			break
		}
	}
	port := status.Peer.Port
	return discoveryMessage{
		Hawk:        1,
		Op:          "announce",
		ID:          identity.PeerID,
		Name:        identity.Name,
		LibraryName: s.base.Lib.LibraryName(),
		PeerPort:    port,
		Writable:    writable,
		TargetLibID: identity.LibraryID,
	}
}

// Discover 广播 probe 并收集 900ms 内的 announce。
func (s *LANSupervisor) Discover() ([]lanPeerDTO, error) {
	status := s.Status()
	if !status.Peer.Active {
		return []lanPeerDTO{}, nil
	}
	conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero, Port: 0})
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	_ = setBroadcast(conn)
	probe, _ := json.Marshal(discoveryMessage{Hawk: 1, Op: "probe", ID: s.base.Auth.Identity.PeerID})
	for _, addr := range broadcastAddresses() {
		_, _ = conn.WriteToUDP(probe, &net.UDPAddr{IP: addr, Port: discoveryPort})
	}

	peers := map[string]lanPeerDTO{}
	buf := make([]byte, 4096)
	deadline := time.Now().Add(900 * time.Millisecond)
	for {
		_ = conn.SetReadDeadline(deadline)
		n, from, err := conn.ReadFromUDP(buf)
		if err != nil {
			break
		}
		var message discoveryMessage
		if err := json.Unmarshal(buf[:n], &message); err != nil || message.Op != "announce" {
			continue
		}
		if message.ID == s.base.Auth.Identity.PeerID || message.PeerPort == 0 {
			continue
		}
		url := fmt.Sprintf("http://%s:%d", from.IP.String(), message.PeerPort)
		peers[message.ID] = lanPeerDTO{
			ID:              message.ID,
			Name:            message.Name,
			LibraryName:     message.LibraryName,
			URL:             url,
			Writable:        message.Writable,
			RequiresPairing: true,
			TargetLibraryID: message.TargetLibID,
		}
	}
	out := make([]lanPeerDTO, 0, len(peers))
	for _, peer := range peers {
		out = append(out, peer)
	}
	return out, nil
}

func broadcastAddresses() []net.IP {
	addresses := []net.IP{net.IPv4bcast}
	interfaces, err := net.Interfaces()
	if err != nil {
		return addresses
	}
	for _, iface := range interfaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, addr := range addrs {
			ipNet, ok := addr.(*net.IPNet)
			if !ok || ipNet.IP.To4() == nil {
				continue
			}
			ip := ipNet.IP.To4()
			mask := ipNet.Mask
			if len(mask) != net.IPv4len {
				continue
			}
			broadcast := make(net.IP, net.IPv4len)
			for i := 0; i < net.IPv4len; i++ {
				broadcast[i] = ip[i] | ^mask[i]
			}
			addresses = append(addresses, broadcast)
		}
	}
	return addresses
}

// ---------- DTO ----------

type lanAccessKeyDTO struct {
	ID          string       `json:"id"`
	Label       string       `json:"label"`
	AccessKey   string       `json:"access_key"`
	Role        library.Role `json:"role"`
	CreatedAtMS int64        `json:"created_at_ms"`
	PeerID      *string      `json:"peer_id,omitempty"`
	LibraryName *string      `json:"library_name,omitempty"`
	URL         *string      `json:"url,omitempty"`
}

type lanSettingsDTO struct {
	Enabled         bool              `json:"enabled"`
	Port            int               `json:"port"`
	PeerPort        int               `json:"peer_port"`
	PeerID          string            `json:"peer_id"`
	TargetLibraryID string            `json:"target_library_id"`
	Name            string            `json:"name"`
	LibraryName     string            `json:"library_name"`
	Keys            []lanAccessKeyDTO `json:"keys"`
	PeerKeys        []lanAccessKeyDTO `json:"peer_keys"`
	Active          bool              `json:"active"`
	Error           string            `json:"error,omitempty"`
	PeerActive      bool              `json:"peer_active"`
	PeerError       string            `json:"peer_error,omitempty"`
}

type lanClientDTO struct {
	KeyID       string       `json:"key_id"`
	Label       string       `json:"label"`
	LastSeenMS  int64        `json:"last_seen_ms"`
	Online      bool         `json:"online"`
	Role        library.Role `json:"role"`
	PeerID      *string      `json:"peer_id,omitempty"`
	LibraryName *string      `json:"library_name,omitempty"`
	URL         *string      `json:"url,omitempty"`
}

type lanPeerDTO struct {
	ID              string `json:"id"`
	Name            string `json:"name"`
	LibraryName     string `json:"library_name"`
	URL             string `json:"url"`
	Writable        bool   `json:"writable"`
	RequiresPairing bool   `json:"requires_pairing"`
	TargetLibraryID string `json:"target_library_id"`
}

func (s *LANSupervisor) settingsDTO() lanSettingsDTO {
	config := s.base.Lib.Config.Current()
	status := s.Status()
	identity := s.base.Auth.Identity
	keys := s.base.Auth.Keys.List(identity.LibraryID)
	keyDTOs := make([]lanAccessKeyDTO, 0, len(keys))
	seen := make(map[string]struct{}, len(keys))
	for _, key := range keys {
		if _, ok := seen[key.ID]; ok {
			continue
		}
		seen[key.ID] = struct{}{}
		dto := lanAccessKeyDTO{
			ID:          key.ID,
			Label:       key.Label,
			Role:        key.Role,
			CreatedAtMS: key.CreatedAtMS,
		}
		if key.PeerID != "" {
			v := key.PeerID
			dto.PeerID = &v
		}
		if key.LibraryName != "" {
			v := key.LibraryName
			dto.LibraryName = &v
		}
		if key.URL != "" {
			v := key.URL
			dto.URL = &v
		}
		keyDTOs = append(keyDTOs, dto)
	}
	return lanSettingsDTO{
		Enabled:         config.Web.Enabled,
		Port:            config.Web.Port,
		PeerPort:        config.Web.PeerPort,
		PeerID:          identity.PeerID,
		TargetLibraryID: identity.LibraryID,
		Name:            identity.Name,
		LibraryName:     s.base.Lib.LibraryName(),
		Keys:            keyDTOs,
		PeerKeys:        keyDTOs,
		Active:          status.Web.Active,
		Error:           status.Web.Error,
		PeerActive:      status.Peer.Active,
		PeerError:       status.Peer.Error,
	}
}

// ---------- Handlers ----------

func (s *Server) handleLanGet(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	writeOK(w, s.LAN.settingsDTO())
}

func (s *Server) handleLanPut(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Enabled  bool              `json:"enabled"`
		Port     int               `json:"port"`
		PeerPort int               `json:"peer_port"`
		Keys     []lanAccessKeyDTO `json:"keys"`
		PeerKeys []lanAccessKeyDTO `json:"peer_keys"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if body.Port <= 0 || body.Port > 65535 || body.PeerPort <= 0 || body.PeerPort > 65535 {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "端口须为 1–65535")
		return
	}
	if body.Port == body.PeerPort {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "网站端口与直连端口不能相同")
		return
	}
	if body.Enabled && len(s.Auth.Keys.List(s.Auth.Identity.LibraryID)) == 0 {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "启用局域网访问前请至少创建一把访问密钥")
		return
	}
	web := library.WebSettings{Enabled: body.Enabled, Port: body.Port, PeerPort: body.PeerPort}
	if _, err := s.Lib.Config.UpdateWeb(web); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	s.LAN.Rebind()
	writeOK(w, s.LAN.settingsDTO())
}

func (s *Server) handleLanConnectionInfo(w http.ResponseWriter, r *http.Request) {
	identity := s.Auth.Identity
	writeOK(w, map[string]any{
		"protocol":          pairProtocol,
		"peer_id":           identity.PeerID,
		"name":              identity.Name,
		"library_name":      s.Lib.LibraryName(),
		"peer_port":         s.LAN.Status().Peer.Port,
		"target_library_id": identity.LibraryID,
	})
}

func (s *Server) handleLanPeers(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	peers, err := s.LAN.Discover()
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeOK(w, map[string]any{"peers": peers})
}

func (s *Server) handleLanClients(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	writeOK(w, map[string]any{"clients": s.LAN.Clients()})
}

func (s *Server) handleDeviceKeyCreate(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Label       string       `json:"label"`
		Role        library.Role `json:"role"`
		PeerID      string       `json:"peer_id"`
		LibraryName string       `json:"library_name"`
		URL         string       `json:"url"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if strings.TrimSpace(body.Label) == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "设备密钥名称不能为空")
		return
	}
	if body.Role == "" {
		body.Role = library.RoleViewer
	}
	entry, secret, err := s.Auth.Keys.Create(s.Auth.Identity.LibraryID, body.Label, body.Role, "", body.PeerID, body.LibraryName, body.URL)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	s.Lib.Bus.Publish(library.EventLanAccessChanged, map[string]bool{"changed": true})
	if s.LAN != nil {
		s.LAN.Rebind()
	}
	writeOK(w, accessKeyDTO(entry, secret))
}

func (s *Server) handleDeviceKeyUpdate(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Role library.Role `json:"role"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	entry, ok, err := s.Auth.Keys.Patch(r.PathValue("key_id"), "", &body.Role)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !ok {
		writeError(w, http.StatusNotFound, CodeInvalidParam, "设备密钥不存在")
		return
	}
	s.Lib.Bus.Publish(library.EventLanAccessChanged, map[string]bool{"changed": true})
	writeOK(w, accessKeyDTO(entry, ""))
}

func (s *Server) handleDeviceKeyDelete(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	removed, err := s.Auth.Keys.Remove(r.PathValue("key_id"))
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	if !removed {
		writeError(w, http.StatusNotFound, CodeInvalidParam, "设备密钥不存在")
		return
	}
	s.Lib.Bus.Publish(library.EventLanAccessChanged, map[string]bool{"changed": true})
	if s.LAN != nil {
		s.LAN.Rebind()
	}
	writeOK(w, map[string]bool{"deleted": true})
}

func accessKeyDTO(entry auth.KeyEntry, secret string) lanAccessKeyDTO {
	dto := lanAccessKeyDTO{
		ID:          entry.ID,
		Label:       entry.Label,
		AccessKey:   secret,
		Role:        entry.Role,
		CreatedAtMS: entry.CreatedAtMS,
	}
	if entry.PeerID != "" {
		v := entry.PeerID
		dto.PeerID = &v
	}
	if entry.LibraryName != "" {
		v := entry.LibraryName
		dto.LibraryName = &v
	}
	if entry.URL != "" {
		v := entry.URL
		dto.URL = &v
	}
	return dto
}

// ---------- 配对 ----------

func (s *Server) handlePairRequestCreate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ClientName string          `json:"client_name"`
		Role       library.Role    `json:"role"`
		Reverse    *reversePeerDTO `json:"reverse"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if strings.TrimSpace(body.ClientName) == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "client_name 不能为空")
		return
	}
	if body.Role != library.RoleViewer && body.Role != library.RoleEditor {
		body.Role = library.RoleViewer
	}
	ticket, err := s.LAN.createPair(body.ClientName, body.Role, body.Reverse)
	if err != nil {
		writeError(w, http.StatusTooManyRequests, CodeInvalidParam, err.Error())
		return
	}
	writeOK(w, map[string]any{"request_id": ticket.ID, "expires_in_seconds": int(pairTTL.Seconds())})
}

func (s *LANSupervisor) createPair(clientName string, role library.Role, reverse *reversePeerDTO) (*pairTicket, error) {
	s.pairMu.Lock()
	defer s.pairMu.Unlock()
	now := time.Now()
	s.prunePairsLocked(now)
	pending := 0
	for _, ticket := range s.pairs {
		if ticket.State == "pending" && ticket.ExpiresAt.After(now) {
			pending++
		}
	}
	if pending >= maxPending {
		return nil, fmt.Errorf("待确认配对请求过多")
	}
	ticket := &pairTicket{
		ID:         randomRequestID(),
		ClientName: clientName,
		Role:       role,
		CreatedAt:  now,
		ExpiresAt:  now.Add(pairTTL),
		State:      "pending",
		Reverse:    reverse,
	}
	s.pairs[ticket.ID] = ticket
	return ticket, nil
}

func (s *LANSupervisor) prunePairsLocked(now time.Time) {
	for id, ticket := range s.pairs {
		if now.After(ticket.ExpiresAt) && ticket.State == "pending" {
			delete(s.pairs, id)
		}
	}
}

func (s *LANSupervisor) pair(id string) (*pairTicket, bool) {
	s.pairMu.Lock()
	defer s.pairMu.Unlock()
	s.prunePairsLocked(time.Now())
	ticket, ok := s.pairs[id]
	return ticket, ok
}

func (s *Server) handlePairRequestStatus(w http.ResponseWriter, r *http.Request) {
	ticket, ok := s.LAN.pair(r.PathValue("request_id"))
	if !ok {
		writeOK(w, map[string]any{"protocol": pairProtocol, "status": "expired"})
		return
	}
	s.LAN.pairMu.Lock()
	defer s.LAN.pairMu.Unlock()
	payload := map[string]any{
		"protocol": pairProtocol,
		"status":   ticket.State,
	}
	if ticket.State == "approved" {
		payload["access_key"] = ticket.AccessKey
		payload["role"] = ticket.GrantedRole
		payload["peer_id"] = s.Auth.Identity.PeerID
		payload["name"] = s.Auth.Identity.Name
		payload["library_name"] = s.Lib.LibraryName()
		payload["target_library_id"] = s.Auth.Identity.LibraryID
	}
	if ticket.State == "pending" && time.Now().After(ticket.ExpiresAt) {
		ticket.State = "expired"
		payload["status"] = "expired"
	}
	writeOK(w, payload)
}

func (s *Server) handlePairRequestList(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	s.LAN.pairMu.Lock()
	defer s.LAN.pairMu.Unlock()
	now := time.Now()
	out := make([]map[string]any, 0)
	for _, ticket := range s.LAN.pairs {
		if ticket.State != "pending" || ticket.ExpiresAt.Before(now) {
			continue
		}
		payload := map[string]any{
			"id":            ticket.ID,
			"client_name":   ticket.ClientName,
			"role":          ticket.Role,
			"created_at_ms": ticket.CreatedAt.UnixMilli(),
			"expires_at_ms": ticket.ExpiresAt.UnixMilli(),
		}
		if ticket.Reverse != nil {
			payload["reverse"] = ticket.Reverse
		}
		out = append(out, payload)
	}
	writeOK(w, map[string]any{"requests": out})
}

func (s *Server) handlePairRequestDecision(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Allow bool         `json:"allow"`
		Role  library.Role `json:"role"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	ticket, ok := s.LAN.pair(r.PathValue("request_id"))
	if !ok {
		writeError(w, http.StatusNotFound, CodeInvalidParam, "配对请求不存在或已过期")
		return
	}
	s.LAN.pairMu.Lock()
	defer s.LAN.pairMu.Unlock()
	if ticket.State != "pending" {
		writeError(w, http.StatusConflict, CodeInvalidParam, "配对请求已处理")
		return
	}
	if !body.Allow {
		ticket.State = "denied"
		writeOK(w, map[string]any{"status": "denied", "peer": nil})
		return
	}
	if body.Role != library.RoleViewer && body.Role != library.RoleEditor {
		body.Role = library.RoleViewer
	}
	if len(s.Auth.Keys.List(s.Auth.Identity.LibraryID)) >= 32 {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "直连访问密钥已达上限")
		return
	}
	_, secret, err := s.Auth.Keys.Create(
		s.Auth.Identity.LibraryID,
		ticket.ClientName,
		body.Role,
		"",
		reversePeerID(ticket.Reverse),
		reverseLibraryName(ticket.Reverse),
		firstURL(ticket.Reverse),
	)
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	ticket.State = "approved"
	ticket.AccessKey = secret
	ticket.GrantedRole = body.Role
	s.Lib.Bus.Publish(library.EventLanAccessChanged, map[string]bool{"changed": true})
	var peer *lanPairedPeerDTO
	if ticket.Reverse != nil {
		peer = &lanPairedPeerDTO{
			ID:              ticket.Reverse.PeerID,
			Name:            ticket.Reverse.Name,
			LibraryName:     ticket.Reverse.LibraryName,
			URL:             firstURL(ticket.Reverse),
			Role:            ticket.Reverse.Role,
			PeerKey:         ticket.Reverse.PeerKey,
			TargetLibraryID: ticket.Reverse.TargetLibraryID,
		}
	}
	writeOK(w, map[string]any{"status": "approved", "peer": peer})
}

type lanPairedPeerDTO struct {
	ID              string       `json:"id"`
	Name            string       `json:"name"`
	LibraryName     string       `json:"library_name"`
	URL             string       `json:"url"`
	Role            library.Role `json:"role"`
	PeerKey         string       `json:"peer_key"`
	TargetLibraryID string       `json:"target_library_id"`
}

func reversePeerID(reverse *reversePeerDTO) string {
	if reverse == nil {
		return ""
	}
	return reverse.PeerID
}
func reverseLibraryName(reverse *reversePeerDTO) string {
	if reverse == nil {
		return ""
	}
	return reverse.LibraryName
}
func firstURL(reverse *reversePeerDTO) string {
	if reverse == nil || len(reverse.URLs) == 0 {
		return ""
	}
	return reverse.URLs[0]
}

func randomRequestID() string {
	buf := make([]byte, 16)
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}
