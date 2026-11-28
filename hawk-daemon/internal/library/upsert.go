package library

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// UpsertResult 是单文件入库的结果。
type UpsertResult struct {
	Hash    string
	Created bool
	Changed bool
	DTO     ItemDTO
}

// UpsertFile 让单个文件进入索引。knownHash 可跳过重算（API 导入已算过哈希）。
func (l *Library) UpsertFile(abs, knownHash string, forceHash bool) (*UpsertResult, error) {
	info, err := os.Stat(abs)
	if err != nil {
		return nil, fmt.Errorf("文件不存在: %s", abs)
	}
	if info.IsDir() {
		return nil, fmt.Errorf("不是文件: %s", abs)
	}
	rel, ok := l.Paths.ToRelative(abs)
	if !ok {
		return nil, fmt.Errorf("文件不在素材库内: %s", abs)
	}
	hash := knownHash
	if hash == "" || forceHash {
		hash, err = hashFile(abs)
		if err != nil {
			return nil, fmt.Errorf("计算哈希失败: %w", err)
		}
	}

	file := scanFile{Rel: rel, Abs: abs, Size: info.Size(), Mtime: UnixMillis(info.ModTime())}
	l.writeMu.Lock()
	created, changed, id := l.applyFileLocked(file, hash)
	var item *Item
	if id != "" {
		item, _ = l.Index.ItemSnapshot(id)
	}
	if item != nil {
		if err := l.DB.Save(item); err != nil {
			l.writeMu.Unlock()
			return nil, fmt.Errorf("元数据写入失败: %w", err)
		}
	}
	dto, hasDTO := l.Index.GetDTO(id)
	l.writeMu.Unlock()

	if created {
		if hasDTO {
			l.Bus.Publish(EventItemAdded, dto)
		}
		l.enqueueDerive(id, false)
	} else if changed {
		l.publishChanged(id)
		if item != nil && item.Width == 0 {
			l.enqueueDerive(id, false)
		}
	}
	return &UpsertResult{Hash: id, Created: created, Changed: changed, DTO: dto}, nil
}

// DeletePath 处理文件从磁盘消失（外部删除，不经过回收站）。
func (l *Library) DeletePath(abs string) {
	rel, ok := l.Paths.ToRelative(abs)
	if !ok {
		return
	}
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	hash, ok := l.Index.HashByLocation(rel)
	if !ok {
		return
	}
	l.Index.RemoveLocation(rel)
	if !l.Index.Contains(hash) {
		_ = l.DB.Delete(hash)
		l.deleteThumbnails(hash)
		l.publishLocationLoss(hash)
		return
	}
	if item, ok := l.Index.ItemSnapshot(hash); ok {
		_ = l.DB.Save(item)
	}
	l.publishLocationLoss(hash)
}

// MovePath 处理文件移动/重命名（内容不变）。
func (l *Library) MovePath(oldAbs, newAbs string) {
	oldRel, ok := l.Paths.ToRelative(oldAbs)
	if !ok {
		return
	}
	newRel, ok := l.Paths.ToRelative(newAbs)
	if !ok {
		return
	}
	l.writeMu.Lock()
	defer l.writeMu.Unlock()

	hash, ok := l.Index.HashByLocation(oldRel)
	if !ok {
		// 旧路径未索引：按新文件入库。
		go l.UpsertPath(newAbs)
		return
	}
	wasTrash := IsInTrash(oldRel)
	nowTrash := IsInTrash(newRel)
	if !nowTrash && !l.Config.IsFileIncluded(newRel) {
		l.Index.RemoveLocation(oldRel)
		if !l.Index.Contains(hash) {
			_ = l.DB.Delete(hash)
			l.deleteThumbnails(hash)
		}
		l.publishLocationLoss(hash)
		return
	}
	l.Index.MoveLocation(oldRel, newRel)
	if info, err := os.Stat(newAbs); err == nil {
		l.Index.AddOrUpdateLocation(hash, newRel, info.Size(), UnixMillis(info.ModTime()))
	}
	if item, ok := l.Index.ItemSnapshot(hash); ok {
		_ = l.DB.Save(item)
	}
	l.publishTransition(hash, wasTrash, nowTrash)
}

// DirMove 处理目录移动/重命名：批量迁移位置与注册表前缀。
func (l *Library) DirMove(oldAbs, newAbs string) {
	oldRel, ok := l.Paths.ToRelative(oldAbs)
	if !ok {
		return
	}
	newRel, ok := l.Paths.ToRelative(newAbs)
	if !ok {
		return
	}
	if oldRel == newRel {
		return
	}
	l.writeMu.Lock()
	prefix := oldRel + "/"
	touched := make([]string, 0)
	for rel, info := range l.Index.LocationInfo() {
		if rel != oldRel && !strings.HasPrefix(rel, prefix) {
			continue
		}
		newPath := newRel + rel[len(oldRel):]
		l.Index.MoveLocation(rel, newPath)
		touched = append(touched, info.Hash)
	}
	for _, hash := range touched {
		if item, ok := l.Index.ItemSnapshot(hash); ok {
			_ = l.DB.Save(item)
		}
	}
	l.Prefs.RenamePrefix(oldRel, newRel)
	l.Filter.RenameFolderPrefix(oldRel, newRel)
	l.Locks.RenameFolderPrefix(oldRel, newRel)
	l.writeMu.Unlock()

	oldInTrash := IsInTrash(oldRel + "/")
	newInTrash := IsInTrash(newRel + "/")
	for _, hash := range uniqueStrings(touched) {
		l.publishTransition(hash, oldInTrash, newInTrash)
	}
	l.Bus.Publish(EventFolderChanged, map[string]string{"reason": "external"})
	l.RequestRescan(newRel)
}

// UpsertPath 供 watcher 使用：自动处理目录与文件。
func (l *Library) UpsertPath(abs string) {
	info, err := os.Stat(abs)
	if err != nil {
		l.DeletePath(abs)
		return
	}
	if info.IsDir() {
		l.RequestRescan("")
		l.Bus.Publish(EventFolderChanged, map[string]string{"reason": "external"})
		return
	}
	if time.Since(info.ModTime()) < time.Second {
		// 仍在写入：延迟重试，最多 5 次；后续写入事件也会重置。
		l.scheduleUpsert(abs, 1)
		return
	}
	if _, err := l.UpsertFile(abs, "", false); err != nil {
		fmt.Fprintf(os.Stderr, "入库失败 %s: %v\n", abs, err)
	}
}

func (l *Library) scheduleUpsert(abs string, attempt int) {
	if attempt > 5 {
		return
	}
	delay := time.Duration(attempt) * 1200 * time.Millisecond
	go func() {
		select {
		case <-l.stopCh:
			return
		case <-time.After(delay):
		}
		info, err := os.Stat(abs)
		if err != nil {
			l.DeletePath(abs)
			return
		}
		if time.Since(info.ModTime()) < time.Second {
			l.scheduleUpsert(abs, attempt+1)
			return
		}
		if _, err := l.UpsertFile(abs, "", false); err != nil {
			fmt.Fprintf(os.Stderr, "延迟入库失败 %s: %v\n", abs, err)
		}
	}()
}

func uniqueStrings(values []string) []string {
	seen := map[string]struct{}{}
	out := make([]string, 0, len(values))
	for _, value := range values {
		if _, ok := seen[value]; ok {
			continue
		}
		seen[value] = struct{}{}
		out = append(out, value)
	}
	return out
}

// MetadataMutation 是一次元数据修改。
type MetadataMutation func(*Item)

// ApplyMetadata 应用单条元数据修改并落盘。
func (l *Library) ApplyMetadata(hash string, mutate MetadataMutation) (ItemDTO, bool, error) {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	item, ok := l.Index.ItemSnapshot(hash)
	if !ok {
		return ItemDTO{}, false, nil
	}
	before := cloneItem(item)
	mutate(item)
	if itemsEqual(before, item) {
		return ItemDTO{}, false, nil
	}
	l.Index.UpdateItem(hash, func(current *Item) {
		current.URL = item.URL
		current.Tags = append([]string{}, item.Tags...)
		current.Categories = append([]string{}, item.Categories...)
		current.Star = item.Star
		current.Annotation = item.Annotation
	})
	l.Categories.RegisterAll(item.Categories)
	l.Tags.RegisterAll(item.Tags)
	if err := l.DB.Save(item); err != nil {
		return ItemDTO{}, false, err
	}
	dto, _ := l.Index.GetDTO(hash)
	l.publishChanged(hash)
	return dto, true, nil
}

// ApplyMetadataBatch 应用批量修改，返回实际更新数与不存在的 ids。
func (l *Library) ApplyMetadataBatch(hashes []string, mutate MetadataMutation) (int, []string, error) {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	missing := make([]string, 0)
	changed := make([]*Item, 0)
	seen := map[string]struct{}{}
	for _, hash := range hashes {
		if _, ok := seen[hash]; ok {
			continue
		}
		seen[hash] = struct{}{}
		item, ok := l.Index.ItemSnapshot(hash)
		if !ok {
			missing = append(missing, hash)
			continue
		}
		before := cloneItem(item)
		mutate(item)
		if itemsEqual(before, item) {
			continue
		}
		l.Index.UpdateItem(hash, func(current *Item) {
			current.URL = item.URL
			current.Tags = append([]string{}, item.Tags...)
			current.Categories = append([]string{}, item.Categories...)
			current.Star = item.Star
			current.Annotation = item.Annotation
		})
		l.Categories.RegisterAll(item.Categories)
		l.Tags.RegisterAll(item.Tags)
		changed = append(changed, item)
	}
	if len(changed) > 0 {
		if err := l.DB.SaveBatch(changed); err != nil {
			return 0, missing, err
		}
	}
	for start := 0; start < len(changed); start += 1000 {
		end := min(len(changed), start+1000)
		items := make([]ItemDTO, 0, end-start)
		for _, item := range changed[start:end] {
			if dto, ok := l.Index.GetDTO(item.ID); ok {
				items = append(items, dto)
			}
		}
		if len(items) > 0 {
			l.Bus.Publish(EventItemsUpdated, map[string]any{"items": items})
		}
	}
	return len(changed), missing, nil
}

func itemsEqual(a, b *Item) bool {
	if a.URL != b.URL || a.Star != b.Star || a.Annotation != b.Annotation {
		return false
	}
	if !equalStrings(a.Tags, b.Tags) || !equalStrings(a.Categories, b.Categories) {
		return false
	}
	if a.Width != b.Width || a.Height != b.Height || a.PaletteSet != b.PaletteSet {
		return false
	}
	if len(a.Palette) != len(b.Palette) {
		return false
	}
	for i := range a.Palette {
		if a.Palette[i] != b.Palette[i] {
			return false
		}
	}
	return true
}

// ClearTrash 清空回收站索引；物理删除由 API 在本调用之后执行。
func (l *Library) ClearTrash() error {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	hashes := make([]string, 0)
	for _, item := range l.Index.SnapshotItems() {
		hasTrash := false
		for _, loc := range item.Locations {
			if IsInTrash(loc.Path) {
				hasTrash = true
				break
			}
		}
		if hasTrash {
			hashes = append(hashes, item.ID)
		}
	}
	for _, hash := range hashes {
		for _, loc := range l.Index.ItemLocations(hash, boolPtr(true)) {
			l.Index.RemoveLocation(loc.Path)
		}
		if !l.Index.Contains(hash) {
			_ = l.DB.Delete(hash)
			l.deleteThumbnails(hash)
			l.Bus.Publish(EventItemRemoved, map[string]string{"id": hash})
		} else if item, ok := l.Index.ItemSnapshot(hash); ok {
			_ = l.DB.Save(item)
			l.publishChanged(hash)
		}
	}
	l.Prefs.DeletePrefix(strings.TrimSuffix(TrashPrefix, "/"))
	l.Filter.DeleteFolderPrefix(strings.TrimSuffix(TrashPrefix, "/"))
	l.Locks.DeleteFolderPrefix(strings.TrimSuffix(TrashPrefix, "/"))
	return nil
}

func boolPtr(value bool) *bool { return &value }

// CleanupIndex 清除隐藏/ignore/扩展名白名单外/源文件已删的残留位置。
type CleanupStats struct {
	Checked int `json:"checked"`
	Removed int `json:"removed"`
	Hidden  int `json:"hidden"`
	Ignored int `json:"ignored"`
	Missing int `json:"missing"`
}

func (l *Library) CleanupIndex() (CleanupStats, error) {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	stats := CleanupStats{}
	for rel, info := range l.Index.LocationInfo() {
		hash := info.Hash
		if IsInTrash(rel) {
			continue
		}
		stats.Checked++
		reason := ""
		switch {
		case IsHidden(rel):
			reason = "hidden"
		case IsInternal(rel):
			reason = "ignored"
		case !l.Config.IsFileIncluded(rel):
			reason = "ignored"
		default:
			abs, _ := l.Paths.ToAbsolute(rel)
			if info, err := os.Stat(abs); err != nil || info.IsDir() {
				reason = "missing"
			}
		}
		if reason == "" {
			continue
		}
		l.Index.RemoveLocation(rel)
		stats.Removed++
		switch reason {
		case "hidden":
			stats.Hidden++
		case "ignored":
			stats.Ignored++
		case "missing":
			stats.Missing++
		}
		if !l.Index.Contains(hash) {
			_ = l.DB.Delete(hash)
			l.deleteThumbnails(hash)
			l.publishLocationLoss(hash)
		} else if item, ok := l.Index.ItemSnapshot(hash); ok {
			_ = l.DB.Save(item)
			l.publishChanged(hash)
		}
	}
	return stats, nil
}

// RefreshCache 按范围派发派生缓存修复任务，并清理源文件已消失的位置。
func (l *Library) RefreshCache(scope RefreshScope) (dispatched, removed int, err error) {
	hashes := l.Index.HashesInScope(scope)
	for _, hash := range hashes {
		abs, ok := l.Index.MainSourceAbs(hash, l.Paths)
		if !ok {
			continue
		}
		if _, statErr := os.Stat(abs); statErr != nil {
			l.DeletePath(abs)
			removed++
			continue
		}
		l.enqueueDerive(hash, true)
		dispatched++
	}
	return dispatched, removed, nil
}

// FileAbs 返回位置对应的绝对路径。
func (l *Library) FileAbs(rel string) (string, bool) { return l.Paths.ToAbsolute(rel) }

// FileMtime 返回文件修改时间（毫秒）。
func FileMtime(abs string) int64 {
	info, err := os.Stat(abs)
	if err != nil {
		return 0
	}
	return UnixMillis(info.ModTime())
}

// NormalizeFilePath 用于 API 目标路径拼接。
func NormalizeFilePath(folderPath, name, ext string) string {
	name = strings.TrimSpace(name)
	if name == "" {
		return ""
	}
	if ext == "" {
		return filepath.ToSlash(filepath.Join(folderPath, name))
	}
	return filepath.ToSlash(filepath.Join(folderPath, name+"."+ext))
}
