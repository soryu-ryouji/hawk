package library

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// EnsureParentDir 确保文件的父目录存在。
func EnsureParentDir(absFile string) {
	_ = os.MkdirAll(filepath.Dir(absFile), 0o755)
}

// FindFreeTrashPath 返回回收站中可用的绝对路径；同名冲突追加 " (n)" 后缀。
func (l *Library) FindFreeTrashPath(rel string, isDir bool) string {
	for n := 1; ; n++ {
		candidateRel := TrashPrefix + rel
		if n > 1 {
			candidateRel = TrashPrefix + suffixedName(rel, n, isDir)
		}
		abs, ok := l.Paths.ToAbsolute(candidateRel)
		if !ok {
			continue
		}
		if _, err := os.Stat(abs); os.IsNotExist(err) {
			return abs
		}
	}
}

func suffixedName(rel string, n int, isDir bool) string {
	dir := DirOf(rel)
	name := NameOf(rel)
	ext := ExtOf(rel)
	newName := fmt.Sprintf("%s (%d)", name, n)
	if !isDir && ext != "" {
		newName += "." + ext
	}
	if dir == "" {
		return newName
	}
	return dir + "/" + newName
}

// IsValidName 校验文件/文件夹名：非空、不含路径分隔符、不是 . / .. / .hawk。
func IsValidName(name string) bool {
	trimmed := strings.TrimSpace(name)
	return trimmed != "" && !strings.ContainsAny(trimmed, "/\\") &&
		trimmed != "." && trimmed != ".." && trimmed != HawkDirName
}

// DeletePrefix 幂等删除一个目录前缀下的索引位置与目录设置。
func (l *Library) DeletePrefix(rel string) {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	l.deletePrefixLocked(rel)
	l.Prefs.DeletePrefix(rel)
	if l.Filter.DeleteFolderPrefix(rel) {
		l.Bus.Publish(GlobalFilterChangedEvent, l.Filter.Snapshot())
	}
	if l.Locks.DeleteFolderPrefix(rel) {
		l.Bus.Publish(LocksChangedEvent, l.Locks.Snapshot())
	}
	l.Bus.Publish(EventFolderChanged, map[string]string{"reason": "external"})
}

func (l *Library) deletePrefixLocked(rel string) {
	prefix := rel + "/"
	for path, info := range l.Index.LocationInfo() {
		if path != rel && !strings.HasPrefix(path, prefix) {
			continue
		}
		l.Index.RemoveLocation(path)
		if !l.Index.Contains(info.Hash) {
			_ = l.DB.Delete(info.Hash)
			l.deleteThumbnails(info.Hash)
			l.publishLocationLoss(info.Hash)
		} else if item, ok := l.Index.ItemSnapshot(info.Hash); ok {
			_ = l.DB.Save(item)
			l.publishChanged(info.Hash)
		}
	}
}
