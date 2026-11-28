package library

import (
	"sort"
	"strings"
	"sync"
)

// LocationSnapshot 是一个文件位置的不可变快照。
type LocationSnapshot struct {
	Path        string
	LibraryPath string
	InTrash     bool
}

// RefreshScope 是派生缓存刷新的范围。
type RefreshScope struct {
	Kind  string // folder / category / tag / library
	Value string
}

// Index 是内存索引：hash → item，位置路径 → hash。读写由 RWMutex 保护。
type Index struct {
	mu        sync.RWMutex
	byHash    map[string]*Item
	hashByLoc map[string]string
}

// NewIndex 创建空索引。
func NewIndex() *Index {
	return &Index{byHash: map[string]*Item{}, hashByLoc: map[string]string{}}
}

// Hydrate 用 SQLite 读出的条目重建索引。
func (idx *Index) Hydrate(items []*Item) {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	idx.byHash = make(map[string]*Item, len(items))
	idx.hashByLoc = make(map[string]string, len(items))
	for _, item := range items {
		idx.byHash[item.ID] = item
		for _, loc := range item.Locations {
			idx.hashByLoc[loc.Path] = item.ID
		}
	}
}

func (idx *Index) Contains(hash string) bool {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	_, ok := idx.byHash[hash]
	return ok
}

func (idx *Index) GetDTO(hash string) (ItemDTO, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok || len(item.Locations) == 0 {
		return ItemDTO{}, false
	}
	return item.toDTO(!item.hasLibraryLocations())
}

func (idx *Index) GetDTOAt(hash string, path *string) (ItemDTO, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok || len(item.Locations) == 0 {
		return ItemDTO{}, false
	}
	trashView := !item.hasLibraryLocations()
	var loc *PathEntry
	if path == nil {
		loc = item.mainLocation(trashView)
	} else {
		loc = item.locationAt(*path, trashView)
	}
	if loc == nil {
		return ItemDTO{}, false
	}
	return item.toDTOAt(loc, trashView), true
}

func (idx *Index) LockProjection(hash string) ([]string, []string, []string, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return nil, nil, nil, false
	}
	paths := make([]string, 0, len(item.Locations))
	for _, loc := range item.Locations {
		paths = append(paths, loc.Path)
	}
	return paths, item.Categories, item.Tags, true
}

func (idx *Index) DimIsZero(hash string) bool {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	return ok && item.Width == 0
}

func (idx *Index) AddOrUpdateLocation(hash, path string, size, mtime int64) bool {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return false
	}
	for i := range item.Locations {
		if item.Locations[i].Path == path {
			item.Locations[i].Size = size
			item.Locations[i].ModificationTime = mtime
			return false
		}
	}
	item.Locations = append(item.Locations, PathEntry{Path: path, Size: size, ModificationTime: mtime})
	idx.hashByLoc[path] = hash
	return true
}

func (idx *Index) GetOrAddWithLocation(hash, path string, size, mtime int64) (bool, bool) {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	item, exists := idx.byHash[hash]
	created := false
	if !exists {
		item = newItem(hash)
		idx.byHash[hash] = item
		created = true
	}
	added := false
	found := false
	for i := range item.Locations {
		if item.Locations[i].Path == path {
			item.Locations[i].Size = size
			item.Locations[i].ModificationTime = mtime
			found = true
			break
		}
	}
	if !found {
		item.Locations = append(item.Locations, PathEntry{Path: path, Size: size, ModificationTime: mtime})
		idx.hashByLoc[path] = hash
		added = true
	}
	return created, added
}

func (idx *Index) RemoveLocation(path string) (string, bool) {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	hash, ok := idx.hashByLoc[path]
	if !ok {
		return "", false
	}
	delete(idx.hashByLoc, path)
	item, ok := idx.byHash[hash]
	if ok {
		for i := range item.Locations {
			if item.Locations[i].Path == path {
				item.Locations = append(item.Locations[:i], item.Locations[i+1:]...)
				break
			}
		}
		if len(item.Locations) == 0 {
			delete(idx.byHash, hash)
		}
	}
	return hash, true
}

func (idx *Index) MoveLocation(oldPath, newPath string) (string, bool) {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	hash, ok := idx.hashByLoc[oldPath]
	if !ok {
		return "", false
	}
	delete(idx.hashByLoc, oldPath)
	idx.hashByLoc[newPath] = hash
	item, ok := idx.byHash[hash]
	if !ok {
		return hash, false
	}
	for i := range item.Locations {
		if item.Locations[i].Path == oldPath {
			item.Locations[i].Path = newPath
			break
		}
	}
	return hash, true
}

func (idx *Index) FindLocation(hash string, path *string, wantTrash *bool) (LocationSnapshot, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return LocationSnapshot{}, false
	}
	var loc *PathEntry
	if path == nil {
		switch {
		case wantTrash != nil && !*wantTrash:
			for i := range item.Locations {
				if !IsInTrash(item.Locations[i].Path) {
					loc = &item.Locations[i]
					break
				}
			}
		case wantTrash != nil && *wantTrash:
			for i := range item.Locations {
				if IsInTrash(item.Locations[i].Path) {
					loc = &item.Locations[i]
					break
				}
			}
		default:
			loc = item.mainLocation(false)
			if loc == nil {
				loc = item.mainLocation(true)
			}
		}
	} else {
		for i := range item.Locations {
			candidate := &item.Locations[i]
			if wantTrash != nil && IsInTrash(candidate.Path) != *wantTrash {
				continue
			}
			if candidate.Path == *path || TrashToLibraryPath(candidate.Path) == *path {
				loc = candidate
				break
			}
		}
	}
	if loc == nil {
		return LocationSnapshot{}, false
	}
	return LocationSnapshot{
		Path:        loc.Path,
		LibraryPath: TrashToLibraryPath(loc.Path),
		InTrash:     IsInTrash(loc.Path),
	}, true
}

func (idx *Index) MainSourceAbs(hash string, paths *Paths) (string, bool) {
	idx.mu.RLock()
	item, ok := idx.byHash[hash]
	if !ok {
		idx.mu.RUnlock()
		return "", false
	}
	var chosen *PathEntry
	for i := range item.Locations {
		if !IsInTrash(item.Locations[i].Path) {
			chosen = &item.Locations[i]
			break
		}
	}
	if chosen == nil && len(item.Locations) > 0 {
		chosen = &item.Locations[0]
	}
	if chosen == nil {
		idx.mu.RUnlock()
		return "", false
	}
	rel := chosen.Path
	idx.mu.RUnlock()
	return paths.ToAbsolute(rel)
}

func (idx *Index) HashByLocation(path string) (string, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	hash, ok := idx.hashByLoc[path]
	return hash, ok
}

func (idx *Index) ItemLocations(hash string, wantTrash *bool) []LocationSnapshot {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return nil
	}
	out := make([]LocationSnapshot, 0, len(item.Locations))
	for _, loc := range item.Locations {
		if wantTrash != nil && IsInTrash(loc.Path) != *wantTrash {
			continue
		}
		out = append(out, LocationSnapshot{
			Path:        loc.Path,
			LibraryPath: TrashToLibraryPath(loc.Path),
			InTrash:     IsInTrash(loc.Path),
		})
	}
	return out
}

func (idx *Index) Count() int {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	total := 0
	for _, item := range idx.byHash {
		if item.hasLibraryLocations() {
			total++
		}
	}
	return total
}

func (idx *Index) HasLibraryLocation(hash string) bool {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	return ok && item.hasLibraryLocations()
}

func (idx *Index) LibraryLocationCount(hash string) int {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return 0
	}
	count := 0
	for _, loc := range item.Locations {
		if !IsInTrash(loc.Path) {
			count++
		}
	}
	return count
}

// LocationInfo 返回位置路径 → 所属 hash 与文件 stat 快照。
type LocationInfo struct {
	Hash  string
	Size  int64
	Mtime int64
}

func (idx *Index) LocationInfo() map[string]LocationInfo {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	out := make(map[string]LocationInfo, len(idx.hashByLoc))
	for path, hash := range idx.hashByLoc {
		info := LocationInfo{Hash: hash}
		if item, ok := idx.byHash[hash]; ok {
			for _, loc := range item.Locations {
				if loc.Path == path {
					info.Size = loc.Size
					info.Mtime = loc.ModificationTime
					break
				}
			}
		}
		out[path] = info
	}
	return out
}

func (idx *Index) AllLocationPaths() []string {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	out := make([]string, 0, len(idx.hashByLoc))
	for path := range idx.hashByLoc {
		out = append(out, path)
	}
	return out
}

func (idx *Index) LocationsUnder(prefix string) []string {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	out := make([]string, 0)
	for path := range idx.hashByLoc {
		if strings.HasPrefix(path, prefix) {
			out = append(out, path)
		}
	}
	return out
}

func (idx *Index) AllCategories() []string {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	seen := map[string]struct{}{}
	out := make([]string, 0)
	for _, item := range idx.byHash {
		for _, category := range item.Categories {
			if _, ok := seen[category]; !ok {
				seen[category] = struct{}{}
				out = append(out, category)
			}
		}
	}
	SortEntries(out)
	return out
}

func (idx *Index) TagsWithCounts() []TaxonInfo {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	counts := map[string]int{}
	names := map[string]struct{}{}
	for _, item := range idx.byHash {
		n := 0
		for _, loc := range item.Locations {
			if !IsInTrash(loc.Path) {
				n++
			}
		}
		for _, tag := range item.Tags {
			names[tag] = struct{}{}
			if n > 0 {
				counts[tag] += n
			}
		}
	}
	out := make([]TaxonInfo, 0, len(names))
	for name := range names {
		out = append(out, TaxonInfo{Name: name, Count: counts[name]})
	}
	sort.Slice(out, func(i, j int) bool { return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name) })
	return out
}

func (idx *Index) CommonTaxonomy(hashes []string) ([]string, []string) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	var tags, categories []string
	initialized := false
	for _, hash := range hashes {
		item, ok := idx.byHash[hash]
		if !ok {
			continue
		}
		if !initialized {
			tags = append([]string{}, item.Tags...)
			categories = append([]string{}, item.Categories...)
			initialized = true
			continue
		}
		tags = intersect(tags, item.Tags)
		categories = intersect(categories, item.Categories)
	}
	sort.Slice(tags, func(i, j int) bool { return strings.ToLower(tags[i]) < strings.ToLower(tags[j]) })
	sort.Slice(categories, func(i, j int) bool { return strings.ToLower(categories[i]) < strings.ToLower(categories[j]) })
	return tags, categories
}

func (idx *Index) FolderCounts() map[string]int {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	counts := map[string]int{}
	for _, item := range idx.byHash {
		for _, loc := range item.Locations {
			if IsInTrash(loc.Path) {
				continue
			}
			dir := DirOf(loc.Path)
			for {
				counts[dir]++
				if dir == "" {
					break
				}
				dir = DirOf(dir)
			}
		}
	}
	return counts
}

func (idx *Index) CategoryCounts() map[string]int {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	counts := map[string]int{}
	for _, item := range idx.byHash {
		n := 0
		for _, loc := range item.Locations {
			if !IsInTrash(loc.Path) {
				n++
			}
		}
		if n == 0 {
			continue
		}
		for _, category := range item.Categories {
			counts[category] += n
		}
	}
	return counts
}

func (idx *Index) HashesInScope(scope RefreshScope) []string {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	type entry struct {
		hash  string
		width int
	}
	list := make([]entry, 0)
	for _, item := range idx.byHash {
		matched := false
		switch scope.Kind {
		case "library":
			matched = true
		case "folder":
			for _, loc := range item.Locations {
				if !IsInTrash(loc.Path) && locInFolder(loc.Path, scope.Value, false) {
					matched = true
					break
				}
			}
		case "category":
			matched = containsString(item.Categories, scope.Value)
		case "tag":
			matched = containsString(item.Tags, scope.Value)
		}
		if matched {
			list = append(list, entry{hash: item.ID, width: item.Width})
		}
	}
	sort.Slice(list, func(i, j int) bool {
		if list[i].width != list[j].width {
			return list[i].width < list[j].width
		}
		return list[i].hash < list[j].hash
	})
	out := make([]string, len(list))
	for i, e := range list {
		out[i] = e.hash
	}
	return out
}

// UpdateItem 在写锁内修改 item 并返回是否找到。
func (idx *Index) UpdateItem(hash string, mutate func(*Item)) bool {
	idx.mu.Lock()
	defer idx.mu.Unlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return false
	}
	mutate(item)
	return true
}

// ItemSnapshot 返回用于批量迁移等操作的深拷贝。
func (idx *Index) ItemSnapshot(hash string) (*Item, bool) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	item, ok := idx.byHash[hash]
	if !ok {
		return nil, false
	}
	return cloneItem(item), true
}

// SnapshotItems 返回全部 item 的深拷贝，供级联迁移使用。
func (idx *Index) SnapshotItems() []*Item {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	out := make([]*Item, 0, len(idx.byHash))
	for _, item := range idx.byHash {
		out = append(out, cloneItem(item))
	}
	return out
}

func cloneItem(item *Item) *Item {
	clone := newItem(item.ID)
	clone.Locations = append(clone.Locations, item.Locations...)
	clone.URL = item.URL
	clone.Tags = append(clone.Tags, item.Tags...)
	clone.Categories = append(clone.Categories, item.Categories...)
	clone.Star = item.Star
	clone.Annotation = item.Annotation
	clone.Width = item.Width
	clone.Height = item.Height
	clone.PaletteSet = item.PaletteSet
	clone.Palette = append([]PaletteEntry{}, item.Palette...)
	clone.paletteLab = append([]PaletteColor{}, item.paletteLab...)
	return clone
}

// TaxonInfo 是分类/标签计数项。
type TaxonInfo struct {
	Name  string `json:"name"`
	Count int    `json:"count"`
}

func intersect(a, b []string) []string {
	if len(a) == 0 || len(b) == 0 {
		return nil
	}
	set := make(map[string]struct{}, len(b))
	for _, v := range b {
		set[v] = struct{}{}
	}
	out := make([]string, 0, len(a))
	for _, v := range a {
		if _, ok := set[v]; ok {
			out = append(out, v)
		}
	}
	return out
}
