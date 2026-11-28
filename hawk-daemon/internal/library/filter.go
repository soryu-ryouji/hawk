package library

import (
	"os"
	"strings"
	"sync"

	toml "github.com/pelletier/go-toml/v2"
)

// GlobalFilterChangedEvent 是全局隐藏集变更事件名。
const GlobalFilterChangedEvent = "global_filter.changed"

// GlobalFilterSnapshot 是隐藏项完整快照。
type GlobalFilterSnapshot struct {
	Folders    []string `json:"folders"`
	Categories []string `json:"categories"`
	Tags       []string `json:"tags"`
}

// GlobalFilter 管理 .hawk/global_filter.toml。
type GlobalFilter struct {
	file string

	mu         sync.Mutex
	folders    []string
	categories []string
	tags       []string
}

func NewGlobalFilter(paths *Paths) *GlobalFilter {
	g := &GlobalFilter{file: paths.FilterFile}
	g.mu.Lock()
	g.folders, g.categories, g.tags = loadFilter(paths.FilterFile)
	g.mu.Unlock()
	return g
}

func (g *GlobalFilter) Snapshot() GlobalFilterSnapshot {
	g.mu.Lock()
	defer g.mu.Unlock()
	return GlobalFilterSnapshot{
		Folders:    append([]string{}, g.folders...),
		Categories: append([]string{}, g.categories...),
		Tags:       append([]string{}, g.tags...),
	}
}

func (g *GlobalFilter) SetFolder(path string, hidden bool) bool {
	return g.set(&g.folders, path, hidden)
}
func (g *GlobalFilter) SetCategory(name string, hidden bool) bool {
	return g.set(&g.categories, name, hidden)
}
func (g *GlobalFilter) SetTag(name string, hidden bool) bool {
	return g.set(&g.tags, name, hidden)
}

func (g *GlobalFilter) set(list *[]string, name string, hidden bool) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	exists := containsString(*list, name)
	if hidden == exists {
		return false
	}
	if hidden {
		*list = append(*list, name)
		SortEntries(*list)
	} else {
		kept := make([]string, 0, len(*list))
		for _, entry := range *list {
			if entry != name {
				kept = append(kept, entry)
			}
		}
		*list = kept
	}
	g.saveLocked()
	return true
}

func (g *GlobalFilter) RenameFolderPrefix(oldDir, newDir string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	prefix := oldDir + "/"
	changed := false
	hits := make([]string, 0)
	for _, folder := range g.folders {
		if folder == oldDir || strings.HasPrefix(folder, prefix) {
			hits = append(hits, folder)
		}
	}
	for _, hit := range hits {
		moved := newDir + hit[len(oldDir):]
		g.folders = removeString(g.folders, hit)
		if !containsString(g.folders, moved) {
			g.folders = append(g.folders, moved)
		}
		changed = true
	}
	if changed {
		SortEntries(g.folders)
		g.saveLocked()
	}
	return changed
}

func (g *GlobalFilter) DeleteFolderPrefix(dir string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	prefix := dir + "/"
	kept := make([]string, 0, len(g.folders))
	for _, folder := range g.folders {
		if folder != dir && !strings.HasPrefix(folder, prefix) {
			kept = append(kept, folder)
		}
	}
	if len(kept) == len(g.folders) {
		return false
	}
	g.folders = kept
	g.saveLocked()
	return true
}

func (g *GlobalFilter) RenameCategory(oldName, newName string) bool {
	return g.rename(&g.categories, oldName, newName)
}
func (g *GlobalFilter) RenameTag(oldName, newName string) bool {
	return g.rename(&g.tags, oldName, newName)
}
func (g *GlobalFilter) DeleteCategory(name string) bool {
	return g.delete(&g.categories, name)
}
func (g *GlobalFilter) DeleteTag(name string) bool {
	return g.delete(&g.tags, name)
}

func (g *GlobalFilter) rename(list *[]string, oldName, newName string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	if !containsString(*list, oldName) {
		return false
	}
	*list = removeString(*list, oldName)
	if !containsString(*list, newName) {
		*list = append(*list, newName)
	}
	SortEntries(*list)
	g.saveLocked()
	return true
}

func (g *GlobalFilter) delete(list *[]string, name string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	if !containsString(*list, name) {
		return false
	}
	*list = removeString(*list, name)
	g.saveLocked()
	return true
}

func (g *GlobalFilter) Reload() bool {
	data, err := os.ReadFile(g.file)
	if err != nil {
		return false
	}
	var doc map[string]any
	if err := toml.Unmarshal(data, &doc); err != nil {
		return false
	}
	folders := normalizeList(toStringSlice(doc["folders"]))
	categories := normalizeList(toStringSlice(doc["categories"]))
	tags := normalizeList(toStringSlice(doc["tags"]))
	g.mu.Lock()
	defer g.mu.Unlock()
	changed := !equalStrings(g.folders, folders) || !equalStrings(g.categories, categories) || !equalStrings(g.tags, tags)
	if changed {
		g.folders, g.categories, g.tags = folders, categories, tags
	}
	return changed
}

func (g *GlobalFilter) saveLocked() {
	body := formatStringList("folders", g.folders) + "\n" +
		formatStringList("categories", g.categories) + "\n" +
		formatStringList("tags", g.tags) + "\n"
	AtomicWrite(g.file, body)
}

func loadFilter(file string) ([]string, []string, []string) {
	data, err := os.ReadFile(file)
	if err != nil {
		return []string{}, []string{}, []string{}
	}
	var doc map[string]any
	if err := toml.Unmarshal(data, &doc); err != nil {
		return []string{}, []string{}, []string{}
	}
	return normalizeList(toStringSlice(doc["folders"])),
		normalizeList(toStringSlice(doc["categories"])),
		normalizeList(toStringSlice(doc["tags"]))
}

func normalizeList(list []string) []string {
	out := make([]string, 0, len(list))
	seen := map[string]struct{}{}
	for _, raw := range list {
		item := strings.TrimSpace(raw)
		if item == "" {
			continue
		}
		if _, ok := seen[item]; ok {
			continue
		}
		seen[item] = struct{}{}
		out = append(out, item)
	}
	SortEntries(out)
	return out
}

func removeString(list []string, value string) []string {
	out := make([]string, 0, len(list))
	for _, item := range list {
		if item != value {
			out = append(out, item)
		}
	}
	return out
}
