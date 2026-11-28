package library

import (
	"sort"
	"strings"
)

const colorMatchThreshold = 25.0

// QueryResult 是 item/list 的结果。
type QueryResult struct {
	Items     []ItemDTO
	Total     int
	TotalSize int64
}

// Query 执行过滤、排序与分页。过滤为 AND 语义，结果按位置展开。
func (idx *Index) Query(q ItemQuery) QueryResult {
	entries, totalSize := idx.filter(q)
	total := len(entries)
	orderBy := strings.ToLower(strings.TrimSpace(q.OrderBy))
	if orderBy == "" {
		orderBy = "modification_time"
	}
	desc := !strings.EqualFold(q.Order, "asc")
	sortEntries(entries, orderBy, desc)

	offset := max(0, q.Offset)
	limit := q.Limit
	if limit <= 0 {
		limit = 50
	}
	if offset >= len(entries) {
		return QueryResult{Items: []ItemDTO{}, Total: total, TotalSize: totalSize}
	}
	end := min(len(entries), offset+limit)
	items := make([]ItemDTO, 0, end-offset)
	for i := offset; i < end; i++ {
		entry := &entries[i]
		items = append(items, entry.item.toDTOAt(&entry.loc, q.InTrash))
	}
	return QueryResult{Items: items, Total: total, TotalSize: totalSize}
}

// QuerySkeleton 与 Query 同过滤同排序，不分页，只投影布局字段。
func (idx *Index) QuerySkeleton(q ItemQuery) ([]ItemSkeletonDTO, int64) {
	entries, totalSize := idx.filter(q)
	orderBy := strings.ToLower(strings.TrimSpace(q.OrderBy))
	if orderBy == "" {
		orderBy = "modification_time"
	}
	desc := !strings.EqualFold(q.Order, "asc")
	sortEntries(entries, orderBy, desc)
	out := make([]ItemSkeletonDTO, 0, len(entries))
	for _, entry := range entries {
		out = append(out, ItemSkeletonDTO{
			ID:     entry.item.ID,
			Path:   entry.loc.Path,
			Width:  entry.item.Width,
			Height: entry.item.Height,
			Star:   entry.item.Star,
			Size:   entry.loc.Size,
		})
	}
	return out, totalSize
}

type querySnapshot struct {
	ID         string
	Tags       []string
	Categories []string
	Star       int
	Annotation string
	URL        string
	Width      int
	Height     int
	Palette    []PaletteColor
	Locations  []PathEntry
}

func (s *querySnapshot) toDTOAt(loc *PathEntry, trashView bool) ItemDTO {
	paths := make([]string, 0, len(s.Locations))
	folders := make([]string, 0, len(s.Locations))
	seen := map[string]struct{}{}
	for i := range s.Locations {
		l := &s.Locations[i]
		if IsInTrash(l.Path) != trashView {
			continue
		}
		display := l.Path
		if trashView {
			display = TrashToLibraryPath(l.Path)
		}
		paths = append(paths, display)
		dir := DirOf(display)
		if dir != "" {
			if _, ok := seen[dir]; !ok {
				seen[dir] = struct{}{}
				folders = append(folders, dir)
			}
		}
	}
	palette := make([]PaletteColorDTO, 0, len(s.Palette))
	for _, p := range s.Palette {
		palette = append(palette, PaletteColorDTO{Color: ToHex(p.R, p.G, p.B), Percentage: p.Percentage})
	}
	display := loc.Path
	if trashView {
		display = TrashToLibraryPath(loc.Path)
	}
	var url *string
	if s.URL != "" {
		u := s.URL
		url = &u
	}
	var annotation *string
	if s.Annotation != "" {
		a := s.Annotation
		annotation = &a
	}
	return ItemDTO{
		ID:               s.ID,
		Path:             loc.Path,
		Name:             NameOf(display),
		Ext:              ExtOf(display),
		Width:            s.Width,
		Height:           s.Height,
		Size:             loc.Size,
		URL:              url,
		Tags:             append([]string{}, s.Tags...),
		Categories:       append([]string{}, s.Categories...),
		Paths:            paths,
		Folders:          folders,
		Star:             s.Star,
		Annotation:       annotation,
		ModificationTime: loc.ModificationTime,
		Palette:          palette,
	}
}

type queryEntry struct {
	item *querySnapshot
	loc  PathEntry
}

func (idx *Index) filter(q ItemQuery) ([]queryEntry, int64) {
	idx.mu.RLock()
	defer idx.mu.RUnlock()

	ids := make(map[string]struct{}, len(q.IDs))
	for _, id := range q.IDs {
		ids[id] = struct{}{}
	}

	entries := make([]queryEntry, 0)
	var totalSize int64
	for _, item := range idx.byHash {
		if len(ids) > 0 {
			if _, ok := ids[item.ID]; !ok {
				continue
			}
		}
		if len(q.Tags) > 0 {
			ok := true
			for _, tag := range q.Tags {
				if !containsString(item.Tags, tag) {
					ok = false
					break
				}
			}
			if !ok {
				continue
			}
		}
		if q.Star != nil && item.Star != *q.Star {
			continue
		}
		if len(q.Categories) > 0 {
			matchAll := strings.EqualFold(q.CategoriesMatch, "all")
			if matchAll {
				ok := true
				for _, category := range q.Categories {
					if !containsString(item.Categories, category) {
						ok = false
						break
					}
				}
				if !ok {
					continue
				}
			} else {
				ok := false
				for _, category := range q.Categories {
					if containsString(item.Categories, category) {
						ok = true
						break
					}
				}
				if !ok {
					continue
				}
			}
		}
		if len(q.ExcludeCategories) > 0 {
			excluded := false
			for _, category := range q.ExcludeCategories {
				if containsString(item.Categories, category) {
					excluded = true
					break
				}
			}
			if excluded {
				continue
			}
		}
		if len(q.ExcludeTags) > 0 {
			excluded := false
			for _, tag := range q.ExcludeTags {
				if containsString(item.Tags, tag) {
					excluded = true
					break
				}
			}
			if excluded {
				continue
			}
		}
		if q.WithoutCategories && len(item.Categories) > 0 {
			continue
		}
		if q.WithoutTags && len(item.Tags) > 0 {
			continue
		}
		if q.Annotation != "" && !strings.Contains(strings.ToLower(item.Annotation), strings.ToLower(q.Annotation)) {
			continue
		}
		if q.URL != "" && !strings.Contains(strings.ToLower(item.URL), strings.ToLower(q.URL)) {
			continue
		}
		if q.MinSideGte != nil || q.MinSideLte != nil {
			side := min(item.Width, item.Height)
			if side <= 0 {
				continue
			}
			if q.MinSideGte != nil && side < *q.MinSideGte {
				continue
			}
			if q.MinSideLte != nil && side > *q.MinSideLte {
				continue
			}
		}
		if q.MinWidth != nil || q.MaxWidth != nil {
			if item.Width <= 0 {
				continue
			}
			if q.MinWidth != nil && item.Width < *q.MinWidth {
				continue
			}
			if q.MaxWidth != nil && item.Width > *q.MaxWidth {
				continue
			}
		}
		if q.MinHeight != nil || q.MaxHeight != nil {
			if item.Height <= 0 {
				continue
			}
			if q.MinHeight != nil && item.Height < *q.MinHeight {
				continue
			}
			if q.MaxHeight != nil && item.Height > *q.MaxHeight {
				continue
			}
		}
		if q.Color != nil {
			matched := false
			threshold := colorMatchThreshold * colorMatchThreshold
			for _, p := range item.paletteLab {
				if DeltaESquared(p.Lab, *q.Color) <= threshold {
					matched = true
					break
				}
			}
			if !matched {
				continue
			}
		}

		snap := &querySnapshot{
			ID:         item.ID,
			Tags:       append([]string{}, item.Tags...),
			Categories: append([]string{}, item.Categories...),
			Star:       item.Star,
			Annotation: item.Annotation,
			URL:        item.URL,
			Width:      item.Width,
			Height:     item.Height,
			Palette:    append([]PaletteColor{}, item.paletteLab...),
			Locations:  append([]PathEntry{}, item.Locations...),
		}
		for _, loc := range item.Locations {
			if IsInTrash(loc.Path) != q.InTrash {
				continue
			}
			if !q.LockGuard.IsEmpty() && !q.LockGuard.EntryVisible(loc.Path, item.Categories, item.Tags) {
				continue
			}
			if len(q.Keywords) > 0 {
				ok := true
				display := loc.Path
				if q.InTrash {
					display = TrashToLibraryPath(loc.Path)
				}
				name := strings.ToLower(NameOf(display))
				annotation := strings.ToLower(item.Annotation)
				for _, keyword := range q.Keywords {
					k := strings.ToLower(keyword)
					if !strings.Contains(name, k) && !strings.Contains(annotation, k) {
						ok = false
						break
					}
				}
				if !ok {
					continue
				}
			}
			if len(q.Folders) > 0 {
				ok := false
				for _, folder := range q.Folders {
					if locInFolder(loc.Path, folder, q.FoldersExact) {
						ok = true
						break
					}
				}
				if !ok {
					continue
				}
			}
			if len(q.ExcludeFolders) > 0 {
				excluded := false
				for _, folder := range q.ExcludeFolders {
					if folder != "" && locInFolder(loc.Path, folder, false) {
						excluded = true
						break
					}
				}
				if excluded {
					continue
				}
			}
			if q.Ext != "" && ExtOf(TrashToLibraryPath(loc.Path)) != strings.ToLower(q.Ext) {
				continue
			}
			entries = append(entries, queryEntry{item: snap, loc: loc})
			totalSize += loc.Size
		}
	}
	return entries, totalSize
}

func sortEntries(entries []queryEntry, orderBy string, desc bool) {
	sort.Slice(entries, func(i, j int) bool {
		a, b := &entries[i], &entries[j]
		cmp := 0
		switch orderBy {
		case "name":
			an := strings.ToLower(NameOf(TrashToLibraryPath(a.loc.Path)))
			bn := strings.ToLower(NameOf(TrashToLibraryPath(b.loc.Path)))
			cmp = strings.Compare(an, bn)
		case "size":
			cmp = compareInt64(a.loc.Size, b.loc.Size)
		case "star":
			cmp = compareInt(a.item.Star, b.item.Star)
		default:
			cmp = compareInt64(a.loc.ModificationTime, b.loc.ModificationTime)
		}
		if cmp == 0 {
			cmp = strings.Compare(a.item.ID, b.item.ID)
		}
		if cmp == 0 {
			cmp = strings.Compare(a.loc.Path, b.loc.Path)
		}
		if desc {
			return cmp > 0
		}
		return cmp < 0
	})
}

func compareInt(a, b int) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	default:
		return 0
	}
}

func compareInt64(a, b int64) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	default:
		return 0
	}
}

// locInFolder 判断位置是否在文件夹内。folder 为空表示整个素材库；
// exact 为 true 时只匹配直接位于该目录下的位置。
func locInFolder(locPath, folder string, exact bool) bool {
	dir := DirOf(TrashToLibraryPath(locPath))
	if exact {
		return dir == folder
	}
	return folder == "" || dir == folder || strings.HasPrefix(dir, folder+"/")
}
