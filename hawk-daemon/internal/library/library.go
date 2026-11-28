package library

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// StartupState 是 /app/startup 与就绪网关的共同数据源。
type StartupState struct {
	mu        sync.RWMutex
	status    string // starting / ready / error
	phase     string
	message   string
	processed int
	total     int
	ready     bool
}

func NewStartupState() *StartupState {
	return &StartupState{status: "starting", phase: "sync"}
}

func (s *StartupState) Snapshot() (status, phase, message string, processed, total int) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.status, s.phase, s.message, s.processed, s.total
}

func (s *StartupState) IsReady() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.ready
}

func (s *StartupState) MarkReady() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.status = "ready"
	s.ready = true
	s.phase = ""
	s.message = ""
}

func (s *StartupState) Fail(message string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.status = "error"
	s.message = message
	s.ready = false
}

func (s *StartupState) SetProgress(phase string, processed, total int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.status == "ready" || s.status == "error" {
		return
	}
	s.phase = phase
	s.processed = processed
	s.total = total
}

// ScanStats 是最近一轮全库扫描的统计。
type ScanStats struct {
	StartedUnixMS int64 `json:"started_unix_ms"`
	DurationMS    int64 `json:"duration_ms"`
	Files         int   `json:"files"`
	DirtyDirs     int   `json:"dirty_dirs"`
	Applied       int   `json:"applied"`
}

// Library 是一个打开的素材库。所有可变状态的写入由 writeMu 串行化。
type Library struct {
	Paths  *Paths
	Config *Config
	DB     *MetadataDB
	Index  *Index
	Bus    *EventBus

	Categories *CategoryRegistry
	Tags       *TagRegistry
	Prefs      *ViewPreferences
	Filter     *GlobalFilter
	Locks      *Locks

	Startup *StartupState

	writeMu sync.Mutex
	scanMu  sync.Mutex

	deriveCh     chan deriveJob
	deriveStop   chan struct{}
	deriveWG     sync.WaitGroup
	inflight     map[string]struct{}
	inflightMu   sync.Mutex
	deriveActive atomic.Int32

	taskMu          sync.Mutex
	lastTaskPublish time.Time

	watcher *watcher
	stopCh  chan struct{}
	stopWG  sync.WaitGroup

	suppressMu       sync.Mutex
	suppressedPaths  map[string]time.Time
	suppressedPrefix map[string]time.Time

	// OnWebChanged 由 main 注入：配置 [web] 变化时唤醒局域网监听重绑。
	OnWebChanged func()

	statsMu   sync.Mutex
	lastScan  *ScanStats
	overflow  bool
	thumbnail ThumbnailStats
}

// ThumbnailStats 是缩略图 worker 队列快照。
type ThumbnailStats struct {
	Pending int `json:"pending"`
	Active  int `json:"active"`
}

// OpenLibrary 打开素材库：创建目录、读取配置、注水 SQLite、构建内存索引。
func OpenLibrary(root, cacheParent string) (*Library, error) {
	paths, err := NewPaths(root, cacheParent)
	if err != nil {
		return nil, err
	}
	if info, err := os.Stat(paths.Root); err != nil || !info.IsDir() {
		return nil, fmt.Errorf("素材库目录不存在: %s", paths.Root)
	}
	if locationErr := paths.CacheLocationError(); locationErr != "" {
		return nil, fmt.Errorf("%s", locationErr)
	}
	if err := paths.EnsureLayout(); err != nil {
		return nil, err
	}

	config := NewConfig(paths)
	db, err := OpenMetadataDB(paths.MetadataDB)
	if err != nil {
		return nil, err
	}
	items, err := db.LoadAll()
	if err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("元数据读取失败: %w", err)
	}

	index := NewIndex()
	valid := make([]*Item, 0, len(items))
	for _, item := range items {
		kept := item.Locations[:0]
		for _, loc := range item.Locations {
			if IsHidden(loc.Path) || (!IsInTrash(loc.Path) && !config.IsFileIncluded(loc.Path)) {
				continue
			}
			kept = append(kept, loc)
		}
		if len(kept) == 0 {
			if len(item.Locations) > 0 {
				_ = db.Delete(item.ID)
			}
			continue
		}
		item.Locations = kept
		valid = append(valid, item)
	}
	index.Hydrate(valid)

	lib := &Library{
		Paths:            paths,
		Config:           config,
		DB:               db,
		Index:            index,
		Bus:              NewEventBus(),
		Categories:       NewCategoryRegistry(paths),
		Tags:             NewTagRegistry(paths),
		Prefs:            NewViewPreferences(paths),
		Filter:           NewGlobalFilter(paths),
		Locks:            NewLocks(paths),
		Startup:          NewStartupState(),
		deriveCh:         make(chan deriveJob, 4096),
		deriveStop:       make(chan struct{}),
		inflight:         map[string]struct{}{},
		stopCh:           make(chan struct{}),
		suppressedPaths:  map[string]time.Time{},
		suppressedPrefix: map[string]time.Time{},
	}
	return lib, nil
}

// Close 停止 watcher 与 worker。
func (l *Library) Close() {
	close(l.stopCh)
	close(l.deriveStop)
	l.deriveWG.Wait()
	l.stopWG.Wait()
	if l.watcher != nil {
		l.watcher.Close()
	}
	_ = l.DB.Close()
}

// Start 启动 worker、文件监听与后台扫描；就绪在注水完成后立即置位。
func (l *Library) Start() {
	l.startDeriveWorkers()
	l.startWatcher()
	l.Startup.MarkReady()
	go l.initialScan()
	go l.periodicLoop()
}

func (l *Library) initialScan() {
	if err := l.RunScan(scanOptions{}); err != nil {
		l.Startup.Fail(err.Error())
	}
}

func (l *Library) periodicLoop() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()
	var maintenanceC <-chan time.Time
	var maintenanceTicker *time.Ticker
	if interval := maintenanceInterval(); interval > 0 {
		maintenanceTicker = time.NewTicker(interval)
		maintenanceC = maintenanceTicker.C
		defer maintenanceTicker.Stop()
	}
	var lastFullScan int64
	for {
		select {
		case <-l.stopCh:
			return
		case <-maintenanceC:
			l.runDerivedMaintenance()
		case <-ticker.C:
			settings := l.Config.ScanSettings()
			if !settings.Periodic {
				continue
			}
			l.statsMu.Lock()
			if l.lastScan != nil {
				lastFullScan = l.lastScan.StartedUnixMS
			}
			l.statsMu.Unlock()
			if lastFullScan > 0 && UnixMillis(time.Now())-lastFullScan >= int64(settings.Interval)*1000 {
				go func() { _ = l.RunScan(scanOptions{ForceWalk: true}) }()
			}
		}
	}
}

func maintenanceInterval() time.Duration {
	raw := strings.TrimSpace(os.Getenv("HAWK_MAINTENANCE_INTERVAL"))
	if raw == "0" {
		return 0
	}
	if seconds, err := time.ParseDuration(raw + "s"); err == nil && seconds > 0 {
		return seconds
	}
	return 60 * time.Second
}

// LastScanStats 返回最近一轮扫描统计。
func (l *Library) LastScanStats() *ScanStats {
	l.statsMu.Lock()
	defer l.statsMu.Unlock()
	if l.lastScan == nil {
		return nil
	}
	copyValue := *l.lastScan
	return &copyValue
}

// QueueOverflowed 返回索引队列是否曾溢出。
func (l *Library) QueueOverflowed() bool {
	l.statsMu.Lock()
	defer l.statsMu.Unlock()
	return l.overflow
}

func (l *Library) setQueueOverflow() {
	l.statsMu.Lock()
	l.overflow = true
	l.statsMu.Unlock()
}

func (l *Library) ClearQueueOverflow() {
	l.statsMu.Lock()
	l.overflow = false
	l.statsMu.Unlock()
}

func (l *Library) LibraryName() string {
	config := l.Config.Current()
	if strings.TrimSpace(config.Name) != "" {
		return strings.TrimSpace(config.Name)
	}
	return filepath.Base(l.Paths.Root)
}

func (l *Library) LibraryKey() string { return LibraryKey(l.Paths.Root) }

// TouchStartupProgress 让 app/startup 能看到扫描阶段。
func (l *Library) TouchStartupProgress(phase string, processed, total int) {
	l.Startup.SetProgress(phase, processed, total)
}

// publishChanged 发布 item.updated（锁可见时）。
func (l *Library) publishChanged(hash string) {
	paths, categories, tags, ok := l.Index.LockProjection(hash)
	if !ok {
		return
	}
	if !l.Locks.ZeroGuard().ItemVisible(paths, categories, tags) {
		return
	}
	if dto, ok := l.Index.GetDTO(hash); ok {
		l.Bus.Publish(EventItemUpdated, dto)
	}
}

// publishLocationLoss 在位置被摘除后调用。
func (l *Library) publishLocationLoss(hash string) {
	if !l.Index.Contains(hash) {
		l.Bus.Publish(EventItemRemoved, map[string]string{"id": hash})
		return
	}
	if !l.Index.HasLibraryLocation(hash) {
		l.Bus.Publish(EventItemTrashed, map[string]string{"id": hash})
		return
	}
	l.publishChanged(hash)
}

// publishTransition 在位置进出回收站后调用。
func (l *Library) publishTransition(hash string, wasInTrash, nowInTrash bool) {
	count := l.Index.LibraryLocationCount(hash)
	if !wasInTrash && nowInTrash && count == 0 {
		l.Bus.Publish(EventItemTrashed, map[string]string{"id": hash})
		return
	}
	if wasInTrash && !nowInTrash && count == 1 {
		if dto, ok := l.Index.GetDTO(hash); ok {
			l.Bus.Publish(EventItemRestored, dto)
			return
		}
	}
	l.publishChanged(hash)
}

// SuppressPath 让 watcher 在短时间内忽略该路径的事件（API 已完成索引变更，避免事件抢跑）。
func (l *Library) SuppressPath(abs string, duration time.Duration) {
	if duration <= 0 {
		duration = 2 * time.Second
	}
	abs = filepath.Clean(abs)
	l.suppressMu.Lock()
	l.suppressedPaths[abs] = time.Now().Add(duration)
	l.suppressMu.Unlock()
}

// SuppressPrefix 让 watcher 忽略某目录前缀下的事件。
func (l *Library) SuppressPrefix(abs string, duration time.Duration) {
	if duration <= 0 {
		duration = 2 * time.Second
	}
	abs = filepath.Clean(abs)
	l.suppressMu.Lock()
	l.suppressedPrefix[abs] = time.Now().Add(duration)
	l.suppressMu.Unlock()
}

func (l *Library) isSuppressed(abs string) bool {
	abs = filepath.Clean(abs)
	now := time.Now()
	l.suppressMu.Lock()
	defer l.suppressMu.Unlock()
	for path, until := range l.suppressedPaths {
		if now.After(until) {
			delete(l.suppressedPaths, path)
			continue
		}
		if path == abs {
			return true
		}
	}
	for prefix, until := range l.suppressedPrefix {
		if now.After(until) {
			delete(l.suppressedPrefix, prefix)
			continue
		}
		if abs == prefix || strings.HasPrefix(abs, prefix+string(filepath.Separator)) {
			return true
		}
	}
	return false
}

func (l *Library) scanWorkers() int {
	workers := runtime.NumCPU() / 2
	if workers < 1 {
		workers = 1
	}
	if workers > 16 {
		workers = 16
	}
	return workers
}
