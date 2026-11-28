package library

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	toml "github.com/pelletier/go-toml/v2"
	"golang.org/x/crypto/argon2"
)

// LocksChangedEvent 是锁集变更事件名。
const LocksChangedEvent = "locks.changed"

const (
	throttleMaxFailures = 5
	throttleCooldown    = 60 * time.Second
)

// LockDim 是锁的维度。
type LockDim string

const (
	LockFolder   LockDim = "folder"
	LockCategory LockDim = "category"
	LockTag      LockDim = "tag"
)

func ParseLockDim(s string) (LockDim, bool) {
	switch s {
	case "folder":
		return LockFolder, true
	case "category":
		return LockCategory, true
	case "tag":
		return LockTag, true
	}
	return "", false
}

type lockEntry struct {
	Name     string
	Password string
}

// LocksSnapshot 是锁名称快照。
type LocksSnapshot struct {
	Folders    []string `json:"folders"`
	Categories []string `json:"categories"`
	Tags       []string `json:"tags"`
}

// LockGuard 是请求级「未解锁」锁集合。
type LockGuard struct {
	folders    map[string]struct{}
	categories map[string]struct{}
	tags       map[string]struct{}
}

func (g LockGuard) IsEmpty() bool {
	return len(g.folders) == 0 && len(g.categories) == 0 && len(g.tags) == 0
}

func (g LockGuard) FolderViewLocked(folder string) bool {
	if len(g.folders) == 0 {
		return false
	}
	cur := folder
	for {
		if _, ok := g.folders[cur]; ok {
			return true
		}
		if i := strings.LastIndexByte(cur, '/'); i >= 0 {
			cur = cur[:i]
			continue
		}
		return false
	}
}

func (g LockGuard) CategoryLocked(name string) bool {
	_, ok := g.categories[name]
	return ok
}

func (g LockGuard) TagLocked(name string) bool {
	_, ok := g.tags[name]
	return ok
}

func (g LockGuard) EntryVisible(locPath string, categories, tags []string) bool {
	return g.locationFree(locPath) && g.taxonomyVisible(categories, tags)
}

func (g LockGuard) ItemVisible(locPaths []string, categories, tags []string) bool {
	if g.IsEmpty() {
		return true
	}
	if !g.taxonomyVisible(categories, tags) {
		return false
	}
	for _, path := range locPaths {
		if g.locationFree(path) {
			return true
		}
	}
	return false
}

func (g LockGuard) locationFree(locPath string) bool {
	if len(g.folders) == 0 {
		return true
	}
	cur := DirOf(locPath)
	for {
		if _, ok := g.folders[cur]; ok {
			return false
		}
		if i := strings.LastIndexByte(cur, '/'); i >= 0 {
			cur = cur[:i]
			continue
		}
		return true
	}
}

func (g LockGuard) taxonomyVisible(categories, tags []string) bool {
	if len(g.categories) > 0 {
		for _, category := range categories {
			if _, ok := g.categories[category]; ok {
				return false
			}
		}
	}
	if len(g.tags) > 0 {
		for _, tag := range tags {
			if _, ok := g.tags[tag]; ok {
				return false
			}
		}
	}
	return true
}

// UnlockOutcome 是解锁结果。
type UnlockOutcome int

const (
	UnlockGranted UnlockOutcome = iota
	UnlockNotFound
	UnlockWrongPassword
	UnlockThrottled
)

// ManageOutcome 是设锁/解锁结果。
type ManageOutcome int

const (
	ManageOK ManageOutcome = iota
	ManageNotFound
	ManageWrongOldPassword
	ManageThrottled
)

// Locks 管理 .hawk/locks.toml 与内存解锁票据。
type Locks struct {
	file string

	mu         sync.RWMutex
	folders    []lockEntry
	categories []lockEntry
	tags       []lockEntry
	tickets    map[string]lockTicket

	throttleMu    sync.Mutex
	failures      int
	throttleUntil time.Time
}

type lockTicket struct {
	Dim  LockDim
	Name string
}

func NewLocks(paths *Paths) *Locks {
	lock := &Locks{
		file:    paths.LocksFile,
		tickets: map[string]lockTicket{},
	}
	lock.reloadFromDisk()
	return lock
}

func (l *Locks) Snapshot() LocksSnapshot {
	l.mu.RLock()
	defer l.mu.RUnlock()
	out := LocksSnapshot{
		Folders:    namesOf(l.folders),
		Categories: namesOf(l.categories),
		Tags:       namesOf(l.tags),
	}
	SortEntries(out.Folders)
	SortEntries(out.Categories)
	SortEntries(out.Tags)
	return out
}

func (l *Locks) Contains(dim LockDim, name string) bool {
	l.mu.RLock()
	defer l.mu.RUnlock()
	list := l.listLocked(dim)
	for _, entry := range list {
		if entry.Name == name {
			return true
		}
	}
	return false
}

func (l *Locks) Guard(tickets []string) LockGuard {
	l.mu.RLock()
	defer l.mu.RUnlock()
	unlocked := make(map[lockTicket]struct{}, len(tickets))
	for _, ticket := range tickets {
		if key, ok := l.tickets[ticket]; ok {
			unlocked[key] = struct{}{}
		}
	}
	guard := LockGuard{folders: map[string]struct{}{}, categories: map[string]struct{}{}, tags: map[string]struct{}{}}
	fillGuard(guard.folders, l.folders, LockFolder, unlocked)
	fillGuard(guard.categories, l.categories, LockCategory, unlocked)
	fillGuard(guard.tags, l.tags, LockTag, unlocked)
	return guard
}

func fillGuard(dest map[string]struct{}, entries []lockEntry, dim LockDim, unlocked map[lockTicket]struct{}) {
	for _, entry := range entries {
		if _, ok := unlocked[lockTicket{Dim: dim, Name: entry.Name}]; !ok {
			dest[entry.Name] = struct{}{}
		}
	}
}

func (l *Locks) ZeroGuard() LockGuard { return l.Guard(nil) }

func (l *Locks) Unlock(dim LockDim, name, password string) (UnlockOutcome, string) {
	if wait := l.throttled(); wait {
		return UnlockThrottled, ""
	}
	l.mu.Lock()
	var entry *lockEntry
	switch dim {
	case LockFolder:
		for i := range l.folders {
			if l.folders[i].Name == name {
				entry = &l.folders[i]
				break
			}
		}
	case LockCategory:
		for i := range l.categories {
			if l.categories[i].Name == name {
				entry = &l.categories[i]
				break
			}
		}
	case LockTag:
		for i := range l.tags {
			if l.tags[i].Name == name {
				entry = &l.tags[i]
				break
			}
		}
	}
	if entry == nil {
		l.mu.Unlock()
		return UnlockNotFound, ""
	}
	phc := entry.Password
	l.mu.Unlock()
	if !verifyPassword(phc, password) {
		l.recordFailure()
		return UnlockWrongPassword, ""
	}
	l.clearFailures()
	ticket := randomHex(32)
	l.mu.Lock()
	l.tickets[ticket] = lockTicket{Dim: dim, Name: name}
	l.mu.Unlock()
	return UnlockGranted, ticket
}

func (l *Locks) Set(dim LockDim, name, password string, oldPassword *string) ManageOutcome {
	if l.throttled() {
		return ManageThrottled
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	entries := l.listLocked(dim)
	index := -1
	for i := range entries {
		if entries[i].Name == name {
			index = i
			break
		}
	}
	if index >= 0 {
		if oldPassword == nil || !verifyPassword(entries[index].Password, *oldPassword) {
			l.recordFailureLocked()
			return ManageWrongOldPassword
		}
		l.clearFailuresLocked()
		entries[index].Password = hashPassword(password)
	} else {
		entries = append(entries, lockEntry{Name: name, Password: hashPassword(password)})
	}
	l.setListLocked(dim, entries)
	l.persistLocked()
	return ManageOK
}

func (l *Locks) Remove(dim LockDim, name, password string) ManageOutcome {
	if l.throttled() {
		return ManageThrottled
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	entries := l.listLocked(dim)
	index := -1
	for i := range entries {
		if entries[i].Name == name {
			index = i
			break
		}
	}
	if index < 0 {
		return ManageNotFound
	}
	if !verifyPassword(entries[index].Password, password) {
		l.recordFailureLocked()
		return ManageWrongOldPassword
	}
	l.clearFailuresLocked()
	entries = append(entries[:index], entries[index+1:]...)
	l.setListLocked(dim, entries)
	l.persistLocked()
	return ManageOK
}

func (l *Locks) RenameFolderPrefix(oldDir, newDir string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	prefix := oldDir + "/"
	hits := make([]lockEntry, 0)
	for _, entry := range l.folders {
		if entry.Name == oldDir || strings.HasPrefix(entry.Name, prefix) {
			hits = append(hits, entry)
		}
	}
	if len(hits) == 0 {
		return false
	}
	for _, hit := range hits {
		moved := newDir + hit.Name[len(oldDir):]
		l.folders = removeEntry(l.folders, hit.Name)
		if !hasEntry(l.folders, moved) {
			l.folders = append(l.folders, lockEntry{Name: moved, Password: hit.Password})
		}
	}
	sortLockEntries(l.folders)
	l.persistLocked()
	return true
}

func (l *Locks) DeleteFolderPrefix(dir string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	prefix := dir + "/"
	before := len(l.folders)
	kept := l.folders[:0]
	for _, entry := range l.folders {
		if entry.Name != dir && !strings.HasPrefix(entry.Name, prefix) {
			kept = append(kept, entry)
		}
	}
	l.folders = kept
	if len(l.folders) == before {
		return false
	}
	l.persistLocked()
	return true
}

func (l *Locks) RenameTaxonomy(dim LockDim, oldName, newName string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	entries := l.listLocked(dim)
	index := -1
	for i := range entries {
		if entries[i].Name == oldName {
			index = i
			break
		}
	}
	if index < 0 {
		return false
	}
	if j := indexOfEntry(entries, newName); j >= 0 {
		if j == index {
			return false
		}
		entries = append(entries[:index], entries[index+1:]...)
	} else {
		entries[index].Name = newName
	}
	l.setListLocked(dim, entries)
	l.persistLocked()
	return true
}

func (l *Locks) DeleteTaxonomy(dim LockDim, name string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	entries := l.listLocked(dim)
	before := len(entries)
	entries = removeEntry(entries, name)
	if len(entries) == before {
		return false
	}
	l.setListLocked(dim, entries)
	l.persistLocked()
	return true
}

func (l *Locks) Reload() bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	oldFolders := lockNames(l.folders)
	oldCategories := lockNames(l.categories)
	oldTags := lockNames(l.tags)
	l.reloadFromDiskLocked()
	changed := !equalStrings(oldFolders, lockNames(l.folders)) ||
		!equalStrings(oldCategories, lockNames(l.categories)) ||
		!equalStrings(oldTags, lockNames(l.tags))
	return changed
}

func (l *Locks) listLocked(dim LockDim) []lockEntry {
	switch dim {
	case LockFolder:
		return append([]lockEntry{}, l.folders...)
	case LockCategory:
		return append([]lockEntry{}, l.categories...)
	case LockTag:
		return append([]lockEntry{}, l.tags...)
	}
	return nil
}

func (l *Locks) setListLocked(dim LockDim, entries []lockEntry) {
	sortLockEntries(entries)
	switch dim {
	case LockFolder:
		l.folders = entries
	case LockCategory:
		l.categories = entries
	case LockTag:
		l.tags = entries
	}
}

func (l *Locks) throttled() bool {
	l.throttleMu.Lock()
	defer l.throttleMu.Unlock()
	if l.throttleUntil.IsZero() {
		return false
	}
	if time.Now().Before(l.throttleUntil) {
		return true
	}
	l.throttleUntil = time.Time{}
	l.failures = 0
	return false
}

func (l *Locks) recordFailure() {
	l.throttleMu.Lock()
	defer l.throttleMu.Unlock()
	l.failures++
	if l.failures >= throttleMaxFailures {
		l.throttleUntil = time.Now().Add(throttleCooldown)
	}
}

func (l *Locks) recordFailureLocked() {
	l.throttleMu.Lock()
	defer l.throttleMu.Unlock()
	l.failures++
	if l.failures >= throttleMaxFailures {
		l.throttleUntil = time.Now().Add(throttleCooldown)
	}
}

func (l *Locks) clearFailures() {
	l.throttleMu.Lock()
	defer l.throttleMu.Unlock()
	l.failures = 0
	l.throttleUntil = time.Time{}
}

func (l *Locks) clearFailuresLocked() {
	l.throttleMu.Lock()
	defer l.throttleMu.Unlock()
	l.failures = 0
	l.throttleUntil = time.Time{}
}

func (l *Locks) reloadFromDisk() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.reloadFromDiskLocked()
}

func (l *Locks) reloadFromDiskLocked() {
	l.folders, l.categories, l.tags = loadLocks(l.file)
}

func (l *Locks) persistLocked() {
	_ = saveLocks(l.file, l.folders, l.categories, l.tags)
}

func namesOf(entries []lockEntry) []string {
	out := make([]string, len(entries))
	for i, entry := range entries {
		out[i] = entry.Name
	}
	return out
}

func lockNames(entries []lockEntry) []string { return namesOf(entries) }

func hasEntry(entries []lockEntry, name string) bool { return indexOfEntry(entries, name) >= 0 }

func indexOfEntry(entries []lockEntry, name string) int {
	for i := range entries {
		if entries[i].Name == name {
			return i
		}
	}
	return -1
}

func removeEntry(entries []lockEntry, name string) []lockEntry {
	kept := entries[:0]
	for _, entry := range entries {
		if entry.Name != name {
			kept = append(kept, entry)
		}
	}
	return kept
}

func sortLockEntries(entries []lockEntry) {
	sortSlice(entries, func(a, b lockEntry) bool {
		return strings.ToLower(a.Name) < strings.ToLower(b.Name)
	})
}

func sortSlice[T any](items []T, less func(a, b T) bool) {
	// 小型插入排序即可，避免引入泛型 sort 包装
	for i := 1; i < len(items); i++ {
		for j := i; j > 0 && less(items[j], items[j-1]); j-- {
			items[j], items[j-1] = items[j-1], items[j]
		}
	}
}

// ---------- 密码哈希（Argon2id PHC，参数与 Rust argon2::default 一致） ----------

const (
	argonTime    = 2
	argonMemory  = 19 * 1024
	argonThreads = 1
	argonKeyLen  = 32
	argonSaltLen = 16
)

func hashPassword(password string) string {
	salt := make([]byte, argonSaltLen)
	_, _ = rand.Read(salt)
	hash := argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return fmt.Sprintf("$argon2id$v=19$m=%d,t=%d,p=%d$%s$%s",
		argonMemory, argonTime, argonThreads,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(hash),
	)
}

func verifyPassword(phc, password string) bool {
	parts := strings.Split(phc, "$")
	if len(parts) != 6 || parts[1] != "argon2id" {
		return false
	}
	var memory uint32
	var timeCost uint32
	var threads uint8
	params := strings.Split(parts[3], ",")
	for _, param := range params {
		key, value, ok := strings.Cut(param, "=")
		if !ok {
			return false
		}
		n, err := strconv.ParseUint(value, 10, 32)
		if err != nil {
			return false
		}
		switch key {
		case "m":
			memory = uint32(n)
		case "t":
			timeCost = uint32(n)
		case "p":
			threads = uint8(n)
		}
	}
	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return false
	}
	expected, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return false
	}
	actual := argon2.IDKey([]byte(password), salt, timeCost, memory, threads, uint32(len(expected)))
	return subtle.ConstantTimeCompare(actual, expected) == 1
}

func randomHex(n int) string {
	buf := make([]byte, n)
	_, _ = rand.Read(buf)
	const hexChars = "0123456789abcdef"
	out := make([]byte, n*2)
	for i, b := range buf {
		out[i*2] = hexChars[b>>4]
		out[i*2+1] = hexChars[b&0x0f]
	}
	return string(out)
}

// ---------- 文件读写 ----------

type lockFile struct {
	Folders    []folderLock `toml:"folders"`
	Categories []namedLock  `toml:"categories"`
	Tags       []namedLock  `toml:"tags"`
}

type folderLock struct {
	Path     string `toml:"path"`
	Password string `toml:"password"`
}

type namedLock struct {
	Name     string `toml:"name"`
	Password string `toml:"password"`
}

func loadLocks(file string) ([]lockEntry, []lockEntry, []lockEntry) {
	data, err := os.ReadFile(file)
	if err != nil {
		return nil, nil, nil
	}
	var doc lockFile
	if err := toml.Unmarshal(data, &doc); err != nil {
		return nil, nil, nil
	}
	folders := make([]lockEntry, 0, len(doc.Folders))
	for _, entry := range doc.Folders {
		name := strings.TrimSpace(entry.Path)
		if name == "" || entry.Password == "" {
			continue
		}
		folders = append(folders, lockEntry{Name: name, Password: entry.Password})
	}
	categories := make([]lockEntry, 0, len(doc.Categories))
	for _, entry := range doc.Categories {
		name := strings.TrimSpace(entry.Name)
		if name == "" || entry.Password == "" {
			continue
		}
		categories = append(categories, lockEntry{Name: name, Password: entry.Password})
	}
	tags := make([]lockEntry, 0, len(doc.Tags))
	for _, entry := range doc.Tags {
		name := strings.TrimSpace(entry.Name)
		if name == "" || entry.Password == "" {
			continue
		}
		tags = append(tags, lockEntry{Name: name, Password: entry.Password})
	}
	folders = dedupLockEntries(folders)
	categories = dedupLockEntries(categories)
	tags = dedupLockEntries(tags)
	sortLockEntries(folders)
	sortLockEntries(categories)
	sortLockEntries(tags)
	return folders, categories, tags
}

func dedupLockEntries(entries []lockEntry) []lockEntry {
	seen := map[string]struct{}{}
	out := make([]lockEntry, 0, len(entries))
	for _, entry := range entries {
		if _, ok := seen[entry.Name]; ok {
			continue
		}
		seen[entry.Name] = struct{}{}
		out = append(out, entry)
	}
	return out
}

func saveLocks(file string, folders, categories, tags []lockEntry) error {
	doc := lockFile{
		Folders:    make([]folderLock, 0, len(folders)),
		Categories: make([]namedLock, 0, len(categories)),
		Tags:       make([]namedLock, 0, len(tags)),
	}
	for _, entry := range folders {
		doc.Folders = append(doc.Folders, folderLock{Path: entry.Name, Password: entry.Password})
	}
	for _, entry := range categories {
		doc.Categories = append(doc.Categories, namedLock{Name: entry.Name, Password: entry.Password})
	}
	for _, entry := range tags {
		doc.Tags = append(doc.Tags, namedLock{Name: entry.Name, Password: entry.Password})
	}
	data, err := toml.Marshal(doc)
	if err != nil {
		return err
	}
	AtomicWrite(file, string(data))
	return nil
}
