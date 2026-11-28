package library

import (
	"fmt"
	"os"
	"sort"
	"strings"
	"sync"

	toml "github.com/pelletier/go-toml/v2"
)

var viewOrderBy = map[string]struct{}{
	"modification_time": {},
	"name":              {},
	"size":              {},
	"star":              {},
}

var viewOrder = map[string]struct{}{"asc": {}, "desc": {}}

// ViewSort 是一个视图的排序偏好。
type ViewSort struct {
	OrderBy string `json:"order_by"`
	Order   string `json:"order"`
}

// ViewPreferences 管理 .hawk/view.toml。
type ViewPreferences struct {
	file string

	mu      sync.Mutex
	entries map[string]ViewSort
}

func NewViewPreferences(paths *Paths) *ViewPreferences {
	v := &ViewPreferences{file: paths.ViewFile, entries: map[string]ViewSort{}}
	v.Reload()
	return v
}

func (v *ViewPreferences) Snapshot() map[string]ViewSort {
	v.mu.Lock()
	defer v.mu.Unlock()
	out := make(map[string]ViewSort, len(v.entries))
	for key, value := range v.entries {
		out[key] = value
	}
	return out
}

func (v *ViewPreferences) Set(scope string, sort ViewSort) {
	v.mu.Lock()
	defer v.mu.Unlock()
	v.entries[scope] = sort
	v.saveLocked()
}

func (v *ViewPreferences) Delete(scope string) {
	v.mu.Lock()
	defer v.mu.Unlock()
	if _, ok := v.entries[scope]; !ok {
		return
	}
	delete(v.entries, scope)
	v.saveLocked()
}

func (v *ViewPreferences) RenamePrefix(oldDir, newDir string) {
	v.mu.Lock()
	defer v.mu.Unlock()
	prefix := "folder:" + oldDir
	keys := make([]string, 0)
	for key := range v.entries {
		if key == prefix || strings.HasPrefix(key, prefix+"/") {
			keys = append(keys, key)
		}
	}
	if len(keys) == 0 {
		return
	}
	for _, key := range keys {
		sortValue := v.entries[key]
		delete(v.entries, key)
		v.entries["folder:"+newDir+strings.TrimPrefix(key, prefix)] = sortValue
	}
	v.saveLocked()
}

func (v *ViewPreferences) DeletePrefix(dir string) bool {
	v.mu.Lock()
	defer v.mu.Unlock()
	prefix := "folder:" + dir
	keys := make([]string, 0)
	for key := range v.entries {
		if key == prefix || strings.HasPrefix(key, prefix+"/") {
			keys = append(keys, key)
		}
	}
	if len(keys) == 0 {
		return false
	}
	for _, key := range keys {
		delete(v.entries, key)
	}
	v.saveLocked()
	return true
}

func (v *ViewPreferences) Reload() {
	data, err := os.ReadFile(v.file)
	if err != nil {
		v.mu.Lock()
		v.entries = map[string]ViewSort{}
		v.mu.Unlock()
		return
	}
	var doc map[string]map[string]any
	if err := toml.Unmarshal(data, &doc); err != nil {
		return
	}
	entries := map[string]ViewSort{}
	for rawScope, section := range doc {
		scope, ok := ParseViewScope(rawScope)
		if !ok {
			continue
		}
		orderBy, _ := section["order_by"].(string)
		order, _ := section["order"].(string)
		sortValue, ok := NormalizeViewSort(orderBy, order)
		if !ok {
			continue
		}
		entries[scope] = sortValue
	}
	v.mu.Lock()
	v.entries = entries
	v.mu.Unlock()
}

func (v *ViewPreferences) saveLocked() {
	keys := make([]string, 0, len(v.entries))
	for key := range v.entries {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, key := range keys {
		sortValue := v.entries[key]
		fmt.Fprintf(&b, "[%q]\n", key)
		fmt.Fprintf(&b, "order_by = %q\n", sortValue.OrderBy)
		fmt.Fprintf(&b, "order = %q\n\n", sortValue.Order)
	}
	AtomicWrite(v.file, b.String())
}

// ParseViewScope 校验并规范化 scope：folder:<路径> / category:<名> / tag:<名>。
func ParseViewScope(raw string) (string, bool) {
	if path, ok := strings.CutPrefix(raw, "folder:"); ok {
		if path != "" && !IsValidLibraryPath(path) {
			return "", false
		}
		return "folder:" + path, true
	}
	if name, ok := strings.CutPrefix(raw, "category:"); ok {
		normalized, ok := NormalizeCategoryName(name)
		if !ok {
			return "", false
		}
		return "category:" + normalized, true
	}
	if name, ok := strings.CutPrefix(raw, "tag:"); ok {
		normalized, ok := NormalizeTagName(name)
		if !ok {
			return "", false
		}
		return "tag:" + normalized, true
	}
	return "", false
}

// NormalizeViewSort 校验 order_by/order 白名单并转小写。
func NormalizeViewSort(orderBy, order string) (ViewSort, bool) {
	ob := strings.ToLower(strings.TrimSpace(orderBy))
	o := strings.ToLower(strings.TrimSpace(order))
	if _, ok := viewOrderBy[ob]; !ok {
		return ViewSort{}, false
	}
	if _, ok := viewOrder[o]; !ok {
		return ViewSort{}, false
	}
	return ViewSort{OrderBy: ob, Order: o}, true
}
