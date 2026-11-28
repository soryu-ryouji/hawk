package library

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	toml "github.com/pelletier/go-toml/v2"
)

// WebSettings 是 .hawk/config.toml 的 [web] 段（访问密钥另存于身份目录 keys.json）。
type WebSettings struct {
	Enabled  bool
	Port     int
	PeerPort int
}

// ScanSettings 是周期兜底扫描设置。
type ScanSettings struct {
	Periodic bool
	Interval int // 秒，下限 60
}

// ConfigSnapshot 是一次配置读取的不可变快照。
type ConfigSnapshot struct {
	Name       string
	Ignore     []string
	Extensions []string
	Scan       ScanSettings
	Web        WebSettings
}

// Config 管理 .hawk/config.toml 的读取、热更新与匹配器。
type Config struct {
	paths *Paths

	mu       sync.RWMutex
	snapshot ConfigSnapshot
	matcher  *IgnoreMatcher
	filter   *ExtensionFilter
	lastErr  string
}

const defaultConfigText = `# hawk 项目配置（.hawk/config.toml，按素材库隔离、随库同步）
# name / ignore 保存即热更（文件监听 Reload）
# [web] 段的端口保存即热生效（局域网监听运行期重绑，无需重启）

# 素材库显示名（缺省为库目录名）
# name = "我的素材库"

# 索引时忽略的路径（不含 "/" 的模式匹配任意深度同名项）
ignore = []

# 可见扩展名白名单：只索引这些后缀的文件（空数组 = 不过滤，全部入库）。
# extensions = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "avif"]

# 周期兜底重扫：监听可能静默丢事件，开启后每 interval 秒强制遍历一次
# （只 stat 不读文件内容，按 size/mtime 复用哈希）
[scan]
periodic = true
interval = 900

# 局域网访问（桌面端设置面板读写）。访问密钥存于应用身份目录，不随素材库同步。
[web]
enabled = false
port = 27372
peer_port = 27375
`

// NewConfig 读取配置；文件缺失视为新库并生成默认配置。
func NewConfig(paths *Paths) *Config {
	ensureDefaultConfig(paths)
	removeLegacyWebSecrets(paths)
	snapshot, err := loadConfig(paths.ConfigFile)
	lastErr := ""
	if err != nil {
		snapshot = ConfigSnapshot{}
		lastErr = err.Error()
	}
	applyDefaults(&snapshot)
	return &Config{
		paths:    paths,
		snapshot: snapshot,
		matcher:  BuildIgnoreMatcher(snapshot.Ignore),
		filter:   BuildExtensionFilter(snapshot.Extensions),
		lastErr:  lastErr,
	}
}

func (c *Config) Current() ConfigSnapshot {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return cloneSnapshot(c.snapshot)
}

func (c *Config) Error() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.lastErr
}

// ConfigChange 记录 reload 前后的差异。
type ConfigChange struct {
	IgnoreChanged     bool
	ExtensionsChanged bool
	ScanChanged       bool
	WebChanged        bool
}

// Reload 重新读取配置；解析失败时保留上次有效配置并记录错误。
func (c *Config) Reload() ConfigChange {
	snapshot, err := loadConfig(c.paths.ConfigFile)
	if err != nil {
		c.mu.Lock()
		c.lastErr = err.Error()
		c.mu.Unlock()
		return ConfigChange{}
	}
	applyDefaults(&snapshot)
	matcher := BuildIgnoreMatcher(snapshot.Ignore)
	filter := BuildExtensionFilter(snapshot.Extensions)

	c.mu.Lock()
	defer c.mu.Unlock()
	old := c.snapshot
	change := ConfigChange{
		IgnoreChanged:     !equalStrings(old.Ignore, snapshot.Ignore),
		ExtensionsChanged: !equalStrings(old.Extensions, snapshot.Extensions),
		ScanChanged:       old.Scan != snapshot.Scan,
		WebChanged:        old.Web != snapshot.Web,
	}
	c.snapshot = snapshot
	c.matcher = matcher
	c.filter = filter
	c.lastErr = ""
	return change
}

func (c *Config) UpdateName(name string) error {
	doc, err := readTOMLMap(c.paths.ConfigFile)
	if err != nil {
		return err
	}
	trimmed := strings.TrimSpace(name)
	if trimmed == "" {
		delete(doc, "name")
	} else {
		doc["name"] = trimmed
	}
	if err := writeTOMLMap(c.paths.ConfigFile, doc); err != nil {
		return err
	}
	c.Reload()
	return nil
}

func (c *Config) UpdateScan(scan ScanSettings) (ConfigChange, error) {
	doc, err := readTOMLMap(c.paths.ConfigFile)
	if err != nil {
		return ConfigChange{}, err
	}
	scan.Interval = max(60, scan.Interval)
	section, _ := doc["scan"].(map[string]any)
	if section == nil {
		section = map[string]any{}
	}
	section["periodic"] = scan.Periodic
	section["interval"] = int64(scan.Interval)
	doc["scan"] = section
	if err := writeTOMLMap(c.paths.ConfigFile, doc); err != nil {
		return ConfigChange{}, err
	}
	return c.Reload(), nil
}

func (c *Config) UpdateWeb(web WebSettings) (ConfigChange, error) {
	doc, err := readTOMLMap(c.paths.ConfigFile)
	if err != nil {
		return ConfigChange{}, err
	}
	section, _ := doc["web"].(map[string]any)
	if section == nil {
		section = map[string]any{}
	}
	section["enabled"] = web.Enabled
	section["port"] = int64(web.Port)
	section["peer_port"] = int64(web.PeerPort)
	doc["web"] = section
	if err := writeTOMLMap(c.paths.ConfigFile, doc); err != nil {
		return ConfigChange{}, err
	}
	change := c.Reload()
	if err := c.Error(); err != "" {
		return ConfigChange{}, fmt.Errorf("%s", err)
	}
	current := c.Current()
	if current.Web != web {
		return ConfigChange{}, fmt.Errorf("配置写回后校验失败：web 段未完整持久化")
	}
	return change, nil
}

func (c *Config) IsIgnored(rel string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.matcher.IsIgnored(rel)
}

func (c *Config) IsExtensionIncluded(rel string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.filter.Allows(rel)
}

func (c *Config) IsFileIncluded(rel string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return !c.matcher.IsIgnored(rel) && c.filter.Allows(rel)
}

func (c *Config) ScanSettings() ScanSettings {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.snapshot.Scan
}

func ensureDefaultConfig(paths *Paths) {
	if _, err := os.Stat(paths.ConfigFile); err == nil {
		return
	}
	_ = os.MkdirAll(paths.HawkDir, 0o755)
	_ = os.WriteFile(paths.ConfigFile, []byte(defaultConfigText), 0o644)
}

func removeLegacyWebSecrets(paths *Paths) {
	doc, err := readTOMLMap(paths.ConfigFile)
	if err != nil {
		return
	}
	web, _ := doc["web"].(map[string]any)
	if web == nil {
		return
	}
	_, hadKeys := web["keys"]
	_, hadPeers := web["peer_keys"]
	if !hadKeys && !hadPeers {
		return
	}
	delete(web, "keys")
	delete(web, "peer_keys")
	doc["web"] = web
	_ = writeTOMLMap(paths.ConfigFile, doc)
}

func loadConfig(file string) (ConfigSnapshot, error) {
	doc, err := readTOMLMap(file)
	if err != nil {
		return ConfigSnapshot{}, err
	}
	snapshot := ConfigSnapshot{
		Scan: ScanSettings{Periodic: true, Interval: 900},
		Web:  WebSettings{Port: 27372, PeerPort: 27375},
	}
	if name, ok := doc["name"].(string); ok {
		snapshot.Name = name
	}
	snapshot.Ignore = toStringSlice(doc["ignore"])
	snapshot.Extensions = toStringSlice(doc["extensions"])

	if scan, ok := doc["scan"].(map[string]any); ok {
		if periodic, ok := scan["periodic"].(bool); ok {
			snapshot.Scan.Periodic = periodic
		}
		if interval, ok := toInt64(scan["interval"]); ok {
			snapshot.Scan.Interval = max(60, int(interval))
		}
	}
	if web, ok := doc["web"].(map[string]any); ok {
		if enabled, ok := web["enabled"].(bool); ok {
			snapshot.Web.Enabled = enabled
		}
		if port, ok := toInt64(web["port"]); ok && port >= 1 && port <= 65535 {
			snapshot.Web.Port = int(port)
		}
		if port, ok := toInt64(web["peer_port"]); ok && port >= 1 && port <= 65535 {
			snapshot.Web.PeerPort = int(port)
		}
	}
	return snapshot, nil
}

func applyDefaults(snapshot *ConfigSnapshot) {
	if snapshot.Scan.Interval <= 0 {
		snapshot.Scan.Interval = 900
	}
	if snapshot.Scan.Interval < 60 {
		snapshot.Scan.Interval = 60
	}
	if snapshot.Web.Port == 0 {
		snapshot.Web.Port = 27372
	}
	if snapshot.Web.PeerPort == 0 {
		snapshot.Web.PeerPort = 27375
	}
}

func readTOMLMap(file string) (map[string]any, error) {
	data, err := os.ReadFile(file)
	if err != nil {
		if os.IsNotExist(err) {
			return map[string]any{}, nil
		}
		return nil, fmt.Errorf("读取配置失败 %s: %w", file, err)
	}
	var doc map[string]any
	if err := toml.Unmarshal(data, &doc); err != nil {
		return nil, fmt.Errorf("配置解析失败 %s: %w", file, err)
	}
	return doc, nil
}

func writeTOMLMap(file string, doc map[string]any) error {
	data, err := toml.Marshal(doc)
	if err != nil {
		return fmt.Errorf("配置序列化失败 %s: %w", file, err)
	}
	tmp := file + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return fmt.Errorf("配置写入失败 %s: %w", tmp, err)
	}
	if err := os.Rename(tmp, file); err != nil {
		_ = os.Remove(tmp)
		return fmt.Errorf("配置替换失败 %s: %w", file, err)
	}
	return nil
}

func toStringSlice(value any) []string {
	list, ok := value.([]any)
	if !ok {
		if stringsList, ok := value.([]string); ok {
			return stringsList
		}
		return nil
	}
	out := make([]string, 0, len(list))
	for _, item := range list {
		if s, ok := item.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

func toInt64(value any) (int64, bool) {
	switch v := value.(type) {
	case int64:
		return v, true
	case int:
		return int64(v), true
	case float64:
		return int64(v), true
	}
	return 0, false
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func cloneSnapshot(s ConfigSnapshot) ConfigSnapshot {
	return ConfigSnapshot{
		Name:       s.Name,
		Ignore:     append([]string{}, s.Ignore...),
		Extensions: append([]string{}, s.Extensions...),
		Scan:       s.Scan,
		Web:        s.Web,
	}
}

// ---------- 扩展名白名单 ----------

type ExtensionFilter struct {
	allowed map[string]struct{}
}

func BuildExtensionFilter(list []string) *ExtensionFilter {
	allowed := make(map[string]struct{}, len(list))
	for _, raw := range list {
		ext := strings.ToLower(strings.TrimPrefix(strings.TrimSpace(raw), "."))
		if ext != "" {
			allowed[ext] = struct{}{}
		}
	}
	return &ExtensionFilter{allowed: allowed}
}

func (f *ExtensionFilter) Allows(rel string) bool {
	if len(f.allowed) == 0 {
		return true
	}
	ext := ExtOf(rel)
	if ext == "" {
		return false
	}
	_, ok := f.allowed[ext]
	return ok
}

// ---------- ignore 匹配器 ----------

type IgnoreMatcher struct {
	patterns [][]string
}

func BuildIgnoreMatcher(patterns []string) *IgnoreMatcher {
	expanded := make([][]string, 0, len(patterns)*2)
	for _, raw := range patterns {
		raw = strings.ToLower(strings.TrimSpace(raw))
		if raw == "" {
			continue
		}
		if strings.Contains(raw, "/") {
			expanded = append(expanded, splitSegments(raw))
			expanded = append(expanded, splitSegments(raw+"/**"))
		} else {
			expanded = append(expanded, splitSegments("**/"+raw))
			expanded = append(expanded, splitSegments("**/"+raw+"/**"))
		}
	}
	return &IgnoreMatcher{patterns: expanded}
}

func (m *IgnoreMatcher) IsIgnored(rel string) bool {
	rel = strings.ToLower(rel)
	segments := splitSegments(rel)
	for _, pattern := range m.patterns {
		if globSegments(pattern, segments) {
			return true
		}
	}
	return false
}

func splitSegments(p string) []string {
	parts := strings.Split(p, "/")
	out := make([]string, 0, len(parts))
	for _, part := range parts {
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}

func globSegments(pattern, path []string) bool {
	p, s := 0, 0
	starP := -1
	starS := 0
	for s < len(path) {
		if p < len(pattern) && (pattern[p] == "**" || segmentMatch(pattern[p], path[s])) {
			if pattern[p] == "**" {
				starP = p
				starS = s
				p++
				continue
			}
			p++
			s++
		} else if starP >= 0 {
			p = starP + 1
			starS++
			s = starS
		} else {
			return false
		}
	}
	for p < len(pattern) && pattern[p] == "**" {
		p++
	}
	return p == len(pattern)
}

func segmentMatch(pattern, text string) bool {
	pr := []rune(pattern)
	tr := []rune(text)
	pi, ti := 0, 0
	starPi := -1
	starTi := 0
	for ti < len(tr) {
		if pi < len(pr) && (pr[pi] == '?' || pr[pi] == tr[ti]) {
			pi++
			ti++
		} else if pi < len(pr) && pr[pi] == '*' {
			starPi = pi
			starTi = ti
			pi++
		} else if starPi >= 0 {
			pi = starPi + 1
			starTi++
			ti = starTi
		} else {
			return false
		}
	}
	for pi < len(pr) && pr[pi] == '*' {
		pi++
	}
	return pi == len(pr)
}

// AtomicWrite 原子写入文本文件（临时文件 + rename）。
func AtomicWrite(file, body string) {
	tmp := filepath.Clean(file) + ".tmp"
	if err := os.WriteFile(tmp, []byte(body), 0o644); err == nil {
		if err := os.Rename(tmp, file); err != nil {
			_ = os.Remove(tmp)
		}
	}
}
