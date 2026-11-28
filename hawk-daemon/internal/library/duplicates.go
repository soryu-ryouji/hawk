package library

import "sort"

// DuplicateGroup 是同一内容 hash 的多个可见库内位置。
type DuplicateGroup struct {
	ID      string
	Entries []ItemDTO
}

// Duplicates 返回按内容 hash 分组的重复位置（组大小降序、组 id 升序）。
func (l *Library) Duplicates(guard LockGuard) []DuplicateGroup {
	groups := make([]DuplicateGroup, 0)
	for _, item := range l.Index.SnapshotItems() {
		locations := make([]PathEntry, 0)
		for _, loc := range item.Locations {
			if IsInTrash(loc.Path) {
				continue
			}
			if !guard.IsEmpty() && !guard.EntryVisible(loc.Path, item.Categories, item.Tags) {
				continue
			}
			locations = append(locations, loc)
		}
		if len(locations) < 2 {
			continue
		}
		entries := make([]ItemDTO, 0, len(locations))
		for i := range locations {
			entries = append(entries, item.toDTOAt(&locations[i], false))
		}
		groups = append(groups, DuplicateGroup{ID: item.ID, Entries: entries})
	}
	sort.Slice(groups, func(i, j int) bool {
		if len(groups[i].Entries) != len(groups[j].Entries) {
			return len(groups[i].Entries) > len(groups[j].Entries)
		}
		return groups[i].ID < groups[j].ID
	})
	return groups
}
