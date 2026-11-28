package library

import (
	"fmt"
	"os"
	"sort"
	"strings"
	"sync"

	toml "github.com/pelletier/go-toml/v2"
)

// NormalizeCategoryName 校验扁平分类名：trim 后非空且不含路径分隔符。
func NormalizeCategoryName(raw string) (string, bool) {
	name := strings.TrimSpace(raw)
	if name == "" || strings.ContainsAny(name, "/\\") {
		return "", false
	}
	return name, true
}

// NormalizeTagName 校验标签名：trim 后非空。
func NormalizeTagName(raw string) (string, bool) {
	name := strings.TrimSpace(raw)
	if name == "" {
		return "", false
	}
	return name, true
}

type registry struct {
	file string
	key  string

	mu      sync.RWMutex
	entries []string
}

func newRegistry(file, key string) *registry {
	r := &registry{file: file, key: key}
	r.reload()
	return r
}

func (r *registry) snapshot() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return append([]string{}, r.entries...)
}

func (r *registry) register(name string) {
	name = strings.TrimSpace(name)
	if name == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, entry := range r.entries {
		if entry == name {
			return
		}
	}
	r.entries = append(r.entries, name)
	SortEntries(r.entries)
	r.saveLocked()
}

func (r *registry) rename(oldName, newName string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	found := false
	for _, entry := range r.entries {
		if entry == oldName {
			found = true
			break
		}
	}
	if !found {
		return
	}
	kept := make([]string, 0, len(r.entries))
	for _, entry := range r.entries {
		if entry != oldName {
			kept = append(kept, entry)
		}
	}
	if !containsString(kept, newName) {
		kept = append(kept, newName)
	}
	r.entries = kept
	SortEntries(r.entries)
	r.saveLocked()
}

func (r *registry) remove(name string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	kept := make([]string, 0, len(r.entries))
	for _, entry := range r.entries {
		if entry != name {
			kept = append(kept, entry)
		}
	}
	if len(kept) == len(r.entries) {
		return
	}
	r.entries = kept
	r.saveLocked()
}

func (r *registry) reload() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.entries = loadRegistry(r.file, r.key)
}

func (r *registry) saveLocked() {
	body := formatStringList(r.key, r.entries) + "\n"
	AtomicWrite(r.file, body)
}

func loadRegistry(file, key string) []string {
	data, err := os.ReadFile(file)
	if err != nil {
		return []string{}
	}
	var doc map[string]any
	if err := toml.Unmarshal(data, &doc); err != nil {
		return []string{}
	}
	list := toStringSlice(doc[key])
	out := make([]string, 0, len(list))
	seen := map[string]struct{}{}
	for _, raw := range list {
		name := strings.TrimSpace(raw)
		if name == "" {
			continue
		}
		if _, ok := seen[name]; ok {
			continue
		}
		seen[name] = struct{}{}
		out = append(out, name)
	}
	SortEntries(out)
	return out
}

func formatStringList(key string, entries []string) string {
	quoted := make([]string, len(entries))
	for i, entry := range entries {
		quoted[i] = fmt.Sprintf("%q", entry)
	}
	return fmt.Sprintf("%s = [%s]", key, strings.Join(quoted, ", "))
}

// CategoryRegistry 管理 .hawk/categories.toml。
type CategoryRegistry struct{ inner *registry }

func NewCategoryRegistry(paths *Paths) *CategoryRegistry {
	return &CategoryRegistry{inner: newRegistry(paths.CategoriesFile, "categories")}
}
func (r *CategoryRegistry) Snapshot() []string { return r.inner.snapshot() }
func (r *CategoryRegistry) Contains(name string) bool {
	r.inner.mu.RLock()
	defer r.inner.mu.RUnlock()
	return containsString(r.inner.entries, name)
}
func (r *CategoryRegistry) Register(name string) { r.inner.register(name) }
func (r *CategoryRegistry) RegisterAll(names []string) {
	for _, name := range names {
		r.inner.register(name)
	}
}
func (r *CategoryRegistry) Rename(oldName, newName string) { r.inner.rename(oldName, newName) }
func (r *CategoryRegistry) Delete(name string)             { r.inner.remove(name) }
func (r *CategoryRegistry) Reload()                        { r.inner.reload() }

// TagRegistry 管理 .hawk/tags.toml。
type TagRegistry struct{ inner *registry }

func NewTagRegistry(paths *Paths) *TagRegistry {
	return &TagRegistry{inner: newRegistry(paths.TagsFile, "tags")}
}
func (r *TagRegistry) Snapshot() []string { return r.inner.snapshot() }
func (r *TagRegistry) Contains(name string) bool {
	r.inner.mu.RLock()
	defer r.inner.mu.RUnlock()
	return containsString(r.inner.entries, name)
}
func (r *TagRegistry) Register(name string) { r.inner.register(name) }
func (r *TagRegistry) RegisterAll(names []string) {
	for _, name := range names {
		r.inner.register(name)
	}
}
func (r *TagRegistry) Rename(oldName, newName string) { r.inner.rename(oldName, newName) }
func (r *TagRegistry) Delete(name string)             { r.inner.remove(name) }
func (r *TagRegistry) Reload()                        { r.inner.reload() }

// TaxonomyCounts 合并注册表与索引赋值，返回稳定排序的计数列表。
func TaxonomyCounts(registry []string, indexCounts map[string]int) []TaxonInfo {
	names := map[string]struct{}{}
	for _, name := range registry {
		names[name] = struct{}{}
	}
	for name := range indexCounts {
		names[name] = struct{}{}
	}
	out := make([]TaxonInfo, 0, len(names))
	for name := range names {
		out = append(out, TaxonInfo{Name: name, Count: indexCounts[name]})
	}
	sort.Slice(out, func(i, j int) bool {
		return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name)
	})
	return out
}
