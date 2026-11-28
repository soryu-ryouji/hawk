package library

import (
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/zeebo/blake3"
)

type scanOptions struct {
	Full      bool
	ForceWalk bool
	Scope     string
}

type scanFile struct {
	Rel   string
	Abs   string
	Size  int64
	Mtime int64
}

type scanResult struct {
	File scanFile
	Hash string
	Err  error
}

// RunScan 执行一轮文件系统扫描。Full=true 时重算全部哈希；Scope 非空时只处理该子树。
func (l *Library) RunScan(opts scanOptions) error {
	l.scanMu.Lock()
	defer l.scanMu.Unlock()

	if opts.Scope != "" {
		if !IsValidLibraryPath(opts.Scope) {
			return fmt.Errorf("非法扫描路径: %s", opts.Scope)
		}
		abs, ok := l.Paths.ToAbsolute(opts.Scope)
		if !ok {
			return fmt.Errorf("非法扫描路径: %s", opts.Scope)
		}
		if info, err := os.Stat(abs); err != nil || !info.IsDir() {
			return fmt.Errorf("文件夹不存在: %s", opts.Scope)
		}
	}

	started := time.Now()
	startedMS := UnixMillis(started)
	l.ClearQueueOverflow()
	l.Startup.SetProgress("scan", 0, 0)
	l.PublishTaskProgress("index", "scan", 0, 0)

	files, walkIncomplete := l.collectFiles(opts.Scope)
	locations := l.Index.LocationInfo()
	seen := make(map[string]struct{}, len(files))
	for _, file := range files {
		seen[file.Rel] = struct{}{}
	}

	todo := make([]scanFile, 0, len(files))
	for _, file := range files {
		if opts.Full {
			todo = append(todo, file)
			continue
		}
		if known, ok := locations[file.Rel]; ok && known.Size == file.Size && known.Mtime == file.Mtime {
			continue
		}
		if time.Since(time.UnixMilli(file.Mtime)) < time.Second {
			// 文件仍在写入：保留索引现状，交给 watcher/下一轮扫描。
			continue
		}
		todo = append(todo, file)
	}
	results := l.hashFiles(todo)
	dirty := make(map[string]*Item)
	added := make([]string, 0)
	updated := make([]string, 0)

	for _, result := range results {
		if result.Err != nil {
			// 读取失败不摘除已有位置；下一轮扫描重试。
			continue
		}
		l.writeMu.Lock()
		created, changed, id := l.applyFileLocked(result.File, result.Hash)
		switch {
		case created:
			added = append(added, id)
			dirty[id] = l.itemCloneLocked(id)
		case changed:
			updated = append(updated, id)
			dirty[id] = l.itemCloneLocked(id)
		}
		l.writeMu.Unlock()
	}

	l.Startup.SetProgress("apply", 0, 0)
	l.PublishTaskProgress("index", "apply", 0, 0)
	if len(dirty) > 0 {
		l.writeMu.Lock()
		ids := make([]string, 0, len(dirty))
		for id := range dirty {
			ids = append(ids, id)
		}
		for start := 0; start < len(ids); start += 500 {
			end := min(len(ids), start+500)
			batch := make([]*Item, 0, end-start)
			for _, id := range ids[start:end] {
				if item, ok := dirty[id]; ok && item != nil {
					batch = append(batch, item)
				}
			}
			if err := l.DB.SaveBatch(batch); err != nil {
				l.writeMu.Unlock()
				return fmt.Errorf("元数据批量写入失败: %w", err)
			}
		}
		l.writeMu.Unlock()
	}

	if !walkIncomplete {
		l.writeMu.Lock()
		for rel := range l.Index.LocationInfo() {
			if opts.Scope != "" && !pathInScope(rel, opts.Scope) {
				continue
			}
			if _, ok := seen[rel]; ok {
				continue
			}
			if hash, ok := l.Index.RemoveLocation(rel); ok {
				if !l.Index.Contains(hash) {
					_ = l.DB.Delete(hash)
					l.deleteThumbnails(hash)
					l.publishLocationLoss(hash)
				} else {
					if item, ok := l.Index.ItemSnapshot(hash); ok {
						_ = l.DB.Save(item)
					}
					l.publishChanged(hash)
				}
			}
		}
		l.writeMu.Unlock()
	}

	if len(added) > 0 {
		l.Bus.Publish(EventItemsAdded, map[string]any{"ids": added})
		for _, id := range added {
			l.enqueueDerive(id, false)
		}
	}
	for start := 0; start < len(updated); start += 1000 {
		end := min(len(updated), start+1000)
		items := make([]ItemDTO, 0, end-start)
		for _, id := range updated[start:end] {
			if dto, ok := l.Index.GetDTO(id); ok {
				items = append(items, dto)
			}
		}
		if len(items) > 0 {
			l.Bus.Publish(EventItemsUpdated, map[string]any{"items": items})
		}
	}
	l.Bus.Publish(EventFolderChanged, map[string]string{"reason": "external"})

	stats := ScanStats{
		StartedUnixMS: startedMS,
		DurationMS:    UnixMillis(time.Now()) - startedMS,
		Files:         len(files),
		Applied:       len(dirty),
	}
	l.statsMu.Lock()
	l.lastScan = &stats
	l.statsMu.Unlock()
	l.Startup.SetProgress("ready", 0, 0)
	l.ClearQueueOverflow()
	l.PublishTaskProgress("index", "", 0, 0)
	return nil
}

func (l *Library) collectFiles(scope string) ([]scanFile, bool) {
	root := l.Paths.Root
	start := root
	if scope != "" {
		if abs, ok := l.Paths.ToAbsolute(scope); ok {
			start = abs
		}
	}
	incomplete := false
	files := make([]scanFile, 0, 1024)
	err := filepath.WalkDir(start, func(abs string, d os.DirEntry, err error) error {
		if err != nil {
			incomplete = true
			if d != nil && d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		rel, ok := l.Paths.ToRelative(abs)
		if !ok {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		if rel == "" {
			return nil
		}
		if rel == internalName {
			return nil
		}
		if strings.HasPrefix(rel, internalName+"/") && !strings.HasPrefix(rel, TrashPrefix) {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		inTrash := strings.HasPrefix(rel, TrashPrefix)
		if d.IsDir() {
			if !inTrash && (IsHidden(rel) || l.Config.IsIgnored(rel)) {
				return filepath.SkipDir
			}
			return nil
		}
		if !d.Type().IsRegular() {
			return nil
		}
		if IsHidden(rel) || IsInternal(rel) {
			return nil
		}
		if !inTrash && !l.Config.IsFileIncluded(rel) {
			return nil
		}
		info, statErr := d.Info()
		if statErr != nil {
			incomplete = true
			return nil
		}
		files = append(files, scanFile{
			Rel:   rel,
			Abs:   abs,
			Size:  info.Size(),
			Mtime: UnixMillis(info.ModTime()),
		})
		return nil
	})
	if err != nil {
		incomplete = true
	}
	return files, incomplete
}

func (l *Library) hashFiles(files []scanFile) []scanResult {
	if len(files) == 0 {
		return nil
	}
	workers := l.scanWorkers()
	jobs := make(chan scanFile)
	results := make(chan scanResult, len(files))
	var wg sync.WaitGroup
	total := len(files)
	processed := 0
	var progressMu sync.Mutex
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for file := range jobs {
				hash, err := hashFile(file.Abs)
				results <- scanResult{File: file, Hash: hash, Err: err}
				progressMu.Lock()
				processed++
				if processed%100 == 0 || processed == total {
					l.Startup.SetProgress("hash", processed, total)
				}
				progressMu.Unlock()
			}
		}()
	}
	go func() {
		for _, file := range files {
			jobs <- file
		}
		close(jobs)
		wg.Wait()
		close(results)
	}()
	out := make([]scanResult, 0, len(files))
	for result := range results {
		out = append(out, result)
	}
	return out
}

func hashFile(abs string) (string, error) {
	f, err := os.Open(abs)
	if err != nil {
		return "", err
	}
	defer f.Close()
	hasher := blake3.New()
	buf := make([]byte, 1024*1024)
	if _, err := io.CopyBuffer(hasher, f, buf); err != nil {
		return "", err
	}
	return hex.EncodeToString(hasher.Sum(nil)), nil
}

// applyFileLocked 把一条文件结果应用到索引。调用方必须持有 writeMu。
func (l *Library) applyFileLocked(file scanFile, hash string) (created, changed bool, id string) {
	if IsHidden(file.Rel) || (IsInternal(file.Rel) && !IsInTrash(file.Rel)) {
		if oldHash, ok := l.Index.HashByLocation(file.Rel); ok {
			l.Index.RemoveLocation(file.Rel)
			l.publishLocationLoss(oldHash)
		}
		return false, false, ""
	}
	if !IsInTrash(file.Rel) && !l.Config.IsFileIncluded(file.Rel) {
		if oldHash, ok := l.Index.HashByLocation(file.Rel); ok {
			l.Index.RemoveLocation(file.Rel)
			l.publishLocationLoss(oldHash)
		}
		return false, false, ""
	}

	existingHash, hasExisting := l.Index.HashByLocation(file.Rel)
	if hasExisting && existingHash == hash {
		item, _ := l.Index.ItemSnapshot(hash)
		locChanged := item != nil && l.locationChanged(item, file.Rel, file.Size, file.Mtime)
		l.Index.AddOrUpdateLocation(hash, file.Rel, file.Size, file.Mtime)
		if item != nil && (locChanged || item.Width == 0) {
			changed = true
		}
		return false, changed, hash
	}

	var oldItem *Item
	if hasExisting {
		oldItem, _ = l.Index.ItemSnapshot(existingHash)
		l.Index.RemoveLocation(file.Rel)
		if !l.Index.Contains(existingHash) {
			_ = l.DB.Delete(existingHash)
			l.deleteThumbnails(existingHash)
			l.publishLocationLoss(existingHash)
		} else {
			l.publishChanged(existingHash)
		}
	}

	createdNow, _ := l.Index.GetOrAddWithLocation(hash, file.Rel, file.Size, file.Mtime)
	if createdNow && oldItem != nil {
		l.Index.UpdateItem(hash, func(item *Item) {
			item.Tags = append([]string{}, oldItem.Tags...)
			item.Categories = append([]string{}, oldItem.Categories...)
			item.Star = oldItem.Star
			item.Annotation = oldItem.Annotation
			item.URL = oldItem.URL
		})
	}
	item, _ := l.Index.ItemSnapshot(hash)
	if item != nil {
		l.Categories.RegisterAll(item.Categories)
		l.Tags.RegisterAll(item.Tags)
	}
	return createdNow, !createdNow, hash
}

func (l *Library) locationChanged(item *Item, rel string, size, mtime int64) bool {
	for _, loc := range item.Locations {
		if loc.Path == rel {
			return loc.Size != size || loc.ModificationTime != mtime
		}
	}
	return true
}

func (l *Library) itemCloneLocked(hash string) *Item {
	item, ok := l.Index.ItemSnapshot(hash)
	if !ok {
		return nil
	}
	return item
}

func pathInScope(rel, scope string) bool {
	return rel == scope || strings.HasPrefix(rel, scope+"/")
}

// requestScan 异步触发一轮扫描。
func (l *Library) RequestScan(full bool) {
	go func() {
		if err := l.RunScan(scanOptions{Full: full}); err != nil {
			l.Startup.Fail(err.Error())
		}
	}()
}

func (l *Library) RequestRescan(scope string) {
	go func() {
		if err := l.RunScan(scanOptions{ForceWalk: true, Scope: scope}); err != nil {
			l.Startup.Fail(err.Error())
		}
	}()
}

// NumCPU 是给状态输出用的稳定并发度。
func NumCPU() int { return runtime.NumCPU() }
