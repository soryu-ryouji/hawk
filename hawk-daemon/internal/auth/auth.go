// Package auth 管理本机 admin token、局域网访问密钥与短期 Session。
// 纯局域网：密钥由目标 daemon 签发，请求经 HTTP Bearer 或 Web Session Cookie 出示。
package auth

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

// KeyEntry 是一把局域网访问密钥（明文只在签发时返回一次）。
type KeyEntry struct {
	ID          string       `json:"id"`
	LibraryID   string       `json:"library_id"`
	Label       string       `json:"label"`
	Hash        string       `json:"hash"`
	Prefix      string       `json:"prefix"`
	Role        library.Role `json:"role"`
	CreatedAtMS int64        `json:"created_at_ms"`
	PeerID      string       `json:"peer_id,omitempty"`
	LibraryName string       `json:"library_name,omitempty"`
	URL         string       `json:"url,omitempty"`
}

// KeyStore 持久化 keys.json；只存 sha256 验证子。
type KeyStore struct {
	file string

	mu      sync.Mutex
	entries []KeyEntry
}

func OpenKeyStore(dir string) (*KeyStore, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, fmt.Errorf("身份目录创建失败 %s: %w", dir, err)
	}
	file := filepath.Join(dir, "keys.json")
	store := &KeyStore{file: file}
	data, err := os.ReadFile(file)
	if err == nil {
		_ = json.Unmarshal(data, &store.entries)
	}
	return store, nil
}

func (s *KeyStore) List(libraryID string) []KeyEntry {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]KeyEntry, 0)
	for _, entry := range s.entries {
		if entry.LibraryID == libraryID {
			out = append(out, entry)
		}
	}
	return out
}

func (s *KeyStore) Get(id string) (KeyEntry, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, entry := range s.entries {
		if entry.ID == id {
			return entry, true
		}
	}
	return KeyEntry{}, false
}

// Create 签发密钥；custom 为空时生成随机 key。返回条目与明文。
func (s *KeyStore) Create(libraryID, label string, role library.Role, custom string, peerID, libraryName, rawURL string) (KeyEntry, string, error) {
	if role != library.RoleViewer && role != library.RoleImporter && role != library.RoleEditor {
		return KeyEntry{}, "", fmt.Errorf("访问密钥仅支持 viewer/importer/editor")
	}
	key := strings.TrimSpace(custom)
	if key == "" {
		key = GenerateKey()
	} else {
		minLen := 4
		if role == library.RoleImporter || role == library.RoleEditor {
			minLen = 12
		}
		if len([]rune(key)) < minLen {
			return KeyEntry{}, "", fmt.Errorf("自定义密钥至少 %d 个字符", minLen)
		}
	}
	hash := HashKey(key)
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, entry := range s.entries {
		if entry.Hash == hash {
			return KeyEntry{}, "", fmt.Errorf("该密钥与现有密钥重复")
		}
	}
	entry := KeyEntry{
		ID:          "k_" + randomHex(8),
		LibraryID:   libraryID,
		Label:       strings.TrimSpace(label),
		Hash:        hash,
		Prefix:      key[:min(4, len(key))],
		Role:        role,
		CreatedAtMS: time.Now().UnixMilli(),
		PeerID:      strings.TrimSpace(peerID),
		LibraryName: strings.TrimSpace(libraryName),
		URL:         strings.TrimSpace(rawURL),
	}
	s.entries = append(s.entries, entry)
	if err := s.saveLocked(); err != nil {
		return KeyEntry{}, "", err
	}
	return entry, key, nil
}

func (s *KeyStore) Patch(id, label string, role *library.Role) (KeyEntry, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.entries {
		if s.entries[i].ID != id {
			continue
		}
		if label != "" {
			s.entries[i].Label = strings.TrimSpace(label)
		}
		if role != nil {
			if *role != library.RoleViewer && *role != library.RoleImporter && *role != library.RoleEditor {
				return KeyEntry{}, false, fmt.Errorf("访问密钥仅支持 viewer/importer/editor")
			}
			s.entries[i].Role = *role
		}
		updated := s.entries[i]
		if err := s.saveLocked(); err != nil {
			return KeyEntry{}, false, err
		}
		return updated, true, nil
	}
	return KeyEntry{}, false, nil
}

func (s *KeyStore) Remove(id string) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.entries {
		if s.entries[i].ID == id {
			s.entries = append(s.entries[:i], s.entries[i+1:]...)
			if err := s.saveLocked(); err != nil {
				return false, err
			}
			return true, nil
		}
	}
	return false, nil
}

// Resolve 用密钥明文验证并返回条目。libraryID 为空表示不限库。
func (s *KeyStore) Resolve(libraryID, credential string) (KeyEntry, bool) {
	hash := HashKey(credential)
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, entry := range s.entries {
		if libraryID != "" && entry.LibraryID != libraryID {
			continue
		}
		if subtle.ConstantTimeCompare([]byte(entry.Hash), []byte(hash)) == 1 {
			return entry, true
		}
	}
	return KeyEntry{}, false
}

func (s *KeyStore) saveLocked() error {
	data, err := json.MarshalIndent(s.entries, "", "  ")
	if err != nil {
		return err
	}
	tmp := s.file + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return fmt.Errorf("访问密钥写入失败: %w", err)
	}
	if err := os.Rename(tmp, s.file); err != nil {
		_ = os.Remove(tmp)
		return fmt.Errorf("访问密钥替换失败: %w", err)
	}
	return nil
}

// HashKey 返回 sha256 十六进制。
func HashKey(key string) string {
	sum := sha256.Sum256([]byte(key))
	return hex.EncodeToString(sum[:])
}

// GenerateKey 生成 hk_ 前缀的 256 位随机密钥。
func GenerateKey() string {
	return "hk_" + randomHex(32)
}

func randomHex(nBytes int) string {
	buf := make([]byte, nBytes)
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}

// ---------- Session ----------

type webSession struct {
	KeyID     string
	LibraryID string
	Expires   time.Time
}

// Sessions 管理 LAN Web 的 HttpOnly Session。
type Sessions struct {
	mu    sync.Mutex
	items map[string]webSession
}

func NewSessions() *Sessions { return &Sessions{items: map[string]webSession{}} }

func (s *Sessions) Create(key KeyEntry) string {
	token := randomHex(32)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items[token] = webSession{KeyID: key.ID, LibraryID: key.LibraryID, Expires: time.Now().Add(30 * 24 * time.Hour)}
	return token
}

func (s *Sessions) Resolve(token, libraryID string) (string, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	session, ok := s.items[token]
	if !ok || session.Expires.Before(time.Now()) || session.LibraryID != libraryID {
		if ok {
			delete(s.items, token)
		}
		return "", false
	}
	return session.KeyID, true
}

func (s *Sessions) Delete(token string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.items, token)
}

// ---------- 请求上下文 ----------

type Principal string

const (
	PrincipalProcessAdmin Principal = "process_admin"
	PrincipalIntegration  Principal = "local_integration"
	PrincipalAccessKey    Principal = "access_key"
)

// AccessContext 是一次请求解析出的授权上下文。
type AccessContext struct {
	Principal Principal
	KeyID     string
	Role      library.Role
	Channel   library.Channel
}

func roleCaps(role library.Role) library.Capability {
	switch role {
	case library.RoleViewer:
		return library.CapRead
	case library.RoleImporter:
		return library.CapRead | library.CapImport
	case library.RoleEditor:
		return library.CapRead | library.CapImport | library.CapEdit
	case library.RoleAdmin:
		return library.CapRead | library.CapImport | library.CapEdit | library.CapManage
	}
	return 0
}

func channelCeiling(channel library.Channel) library.Capability {
	switch channel {
	case library.ChannelLoopback:
		return library.CapRead | library.CapImport | library.CapEdit | library.CapManage
	case library.ChannelLanWeb, library.ChannelLanPeer:
		return library.CapRead | library.CapImport | library.CapEdit
	}
	return 0
}

func (c AccessContext) Capabilities() library.Capability {
	return roleCaps(c.Role) & channelCeiling(c.Channel)
}

func (c AccessContext) Require(cap library.Capability) error {
	if c.Capabilities()&cap != 0 {
		return nil
	}
	return fmt.Errorf("role %s cannot perform %s over %s", c.Role, cap, c.Channel)
}

func (c AccessContext) IsAdmin() bool {
	return c.Role == library.RoleAdmin && c.Channel == library.ChannelLoopback
}

// Identity 是本机在局域网中的稳定标识（不承担密码学身份）。
type Identity struct {
	PeerID      string
	DeviceID    string
	Name        string
	LibraryID   string
	LibraryName string
}

// Service 组合 admin token、AccessKey、Web Session 与本机集成 Session。
type Service struct {
	AdminToken string
	Keys       *KeyStore
	Sessions   *Sessions
	Identity   Identity

	mu           sync.Mutex
	integrations map[string]time.Time
}

func NewService(adminToken, identityDir string, identity Identity) (*Service, error) {
	keys, err := OpenKeyStore(identityDir)
	if err != nil {
		return nil, err
	}
	if adminToken == "" {
		adminToken = randomHex(32)
	}
	return &Service{
		AdminToken:   adminToken,
		Keys:         keys,
		Sessions:     NewSessions(),
		Identity:     identity,
		integrations: map[string]time.Time{},
	}, nil
}

// ResolveCredential 解析 Bearer 凭证；mode 由监听器决定。
func (s *Service) ResolveCredential(credential string, channel library.Channel) (AccessContext, bool) {
	if credential == "" {
		return AccessContext{}, false
	}
	switch channel {
	case library.ChannelLoopback:
		if subtle.ConstantTimeCompare([]byte(credential), []byte(s.AdminToken)) == 1 {
			return AccessContext{Principal: PrincipalProcessAdmin, Role: library.RoleAdmin, Channel: channel}, true
		}
		if s.integrationValid(credential) {
			return AccessContext{Principal: PrincipalIntegration, Role: library.RoleImporter, Channel: channel}, true
		}
	}
	if key, ok := s.Keys.Resolve(s.Identity.LibraryID, credential); ok {
		return AccessContext{Principal: PrincipalAccessKey, KeyID: key.ID, Role: key.Role, Channel: channel}, true
	}
	return AccessContext{}, false
}

func (s *Service) ResolveSession(token string) (string, bool) {
	return s.Sessions.Resolve(token, s.Identity.LibraryID)
}

func (s *Service) SessionAccess(keyID string, channel library.Channel) (AccessContext, bool) {
	key, ok := s.Keys.Get(keyID)
	if !ok {
		return AccessContext{}, false
	}
	return AccessContext{Principal: PrincipalAccessKey, KeyID: key.ID, Role: key.Role, Channel: channel}, true
}

func (s *Service) CreateWebSession(key KeyEntry) string {
	return s.Sessions.Create(key)
}

func (s *Service) DeleteWebSession(token string) { s.Sessions.Delete(token) }

// CreateIntegrationToken 签发本机插件用的短期 Session（不是 admin token）。
func (s *Service) CreateIntegrationToken() string {
	token := randomHex(32)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.integrations[token] = time.Now().Add(30 * time.Minute)
	return token
}

func (s *Service) integrationValid(token string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	expires, ok := s.integrations[token]
	if !ok {
		return false
	}
	if expires.Before(time.Now()) {
		delete(s.integrations, token)
		return false
	}
	return true
}

// BuildIdentity 生成稳定 peer_id / device_id。
func BuildIdentity(hostname, root, libraryID, libraryName string) Identity {
	base := sha256.Sum256([]byte(hostname + "|" + root))
	peerID := hex.EncodeToString(base[:8])
	device := sha256.Sum256([]byte(root))
	return Identity{
		PeerID:      peerID,
		DeviceID:    "d_" + hex.EncodeToString(device[:8]),
		Name:        hostname,
		LibraryID:   libraryID,
		LibraryName: libraryName,
	}
}
