// Package library 承载素材库的全部领域逻辑：路径、配置、SQLite 元数据、内存索引、
// 扫描与监听、缩略图/调色板、分类标签、锁与视图偏好。
//
// 并发模型比 Rust 版简单：一把 RWMutex 保护索引与元数据，写入串行；扫描、监听和缩略图
// 在独立 goroutine 中做 IO/CPU 工作，结果回到锁内应用。API 只读索引投影。
package library

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"
)

const (
	HawkDirName   = ".hawk"
	TrashDirName  = "trash"
	TrashPrefix   = ".hawk/trash/"
	internalName  = ".hawk"
	gitignoreName = ".gitignore"
)

// Paths 是素材库路径布局的唯一权威。root 使用本机绝对路径；库内相对路径一律正斜杠。
type Paths struct {
	Root string

	HawkDir        string
	TrashDir       string
	ConfigFile     string
	MetadataDB     string
	CategoriesFile string
	TagsFile       string
	ViewFile       string
	FilterFile     string
	LocksFile      string

	CacheDir      string
	ThumbnailsDir string
}

// NewPaths 由库根与可选缓存父目录构造路径布局。cacheParent 为空时使用平台默认目录。
func NewPaths(root string, cacheParent string) (*Paths, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, fmt.Errorf("解析素材库路径失败: %w", err)
	}
	abs = filepath.Clean(abs)
	hawkDir := filepath.Join(abs, HawkDirName)

	parent := cacheParent
	if strings.TrimSpace(parent) == "" {
		parent = defaultCacheParent()
	}
	cacheDir := filepath.Join(parent, cacheDirName(abs))

	p := &Paths{
		Root:           abs,
		HawkDir:        hawkDir,
		TrashDir:       filepath.Join(hawkDir, TrashDirName),
		ConfigFile:     filepath.Join(hawkDir, "config.toml"),
		MetadataDB:     filepath.Join(hawkDir, "metadata.db"),
		CategoriesFile: filepath.Join(hawkDir, "categories.toml"),
		TagsFile:       filepath.Join(hawkDir, "tags.toml"),
		ViewFile:       filepath.Join(hawkDir, "view.toml"),
		FilterFile:     filepath.Join(hawkDir, "global_filter.toml"),
		LocksFile:      filepath.Join(hawkDir, "locks.toml"),
		CacheDir:       cacheDir,
		ThumbnailsDir:  filepath.Join(cacheDir, "thumbnails"),
	}
	return p, nil
}

// CacheLocationError 校验缓存目录与库根不互相包含。
func (p *Paths) CacheLocationError() string {
	root := slash(p.Root)
	cache := slash(p.CacheDir)
	if cache == root || strings.HasPrefix(cache, root+"/") {
		return "缓存目录不能位于素材库内"
	}
	if strings.HasPrefix(root, cache+"/") {
		return "素材库不能位于缓存目录内"
	}
	return ""
}

// EnsureLayout 创建 .hawk/ 与缓存目录，并补齐 .gitignore 排除项。
func (p *Paths) EnsureLayout() error {
	for _, dir := range []string{p.HawkDir, p.TrashDir, p.ThumbnailsDir} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return fmt.Errorf("创建目录失败 %s: %w", dir, err)
		}
	}
	gitignore := filepath.Join(p.HawkDir, gitignoreName)
	existing, _ := os.ReadFile(gitignore)
	lines := strings.Split(string(existing), "\n")
	appendLines := make([]string, 0, 2)
	for _, entry := range []string{"trash/", "metadata.db"} {
		found := false
		for _, line := range lines {
			if strings.TrimSpace(line) == entry {
				found = true
				break
			}
		}
		if !found {
			appendLines = append(appendLines, entry)
		}
	}
	if len(appendLines) > 0 {
		f, err := os.OpenFile(gitignore, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
		if err == nil {
			_, _ = f.WriteString(strings.Join(appendLines, "\n") + "\n")
			_ = f.Close()
		}
	}
	return nil
}

// ToRelative 把绝对路径换算成库内正斜杠相对路径；库根返回 ""。
func (p *Paths) ToRelative(abs string) (string, bool) {
	target := filepath.Clean(abs)
	rel, err := filepath.Rel(p.Root, target)
	if err != nil {
		return "", false
	}
	rel = filepath.ToSlash(rel)
	if rel == "." {
		return "", true
	}
	if rel == ".." || strings.HasPrefix(rel, "../") || filepath.IsAbs(rel) {
		return "", false
	}
	return rel, true
}

// ToAbsolute 把库内相对路径换算成绝对路径；越界/绝对路径/空路径返回 false。
func (p *Paths) ToAbsolute(rel string) (string, bool) {
	if !IsValidLibraryPath(rel) {
		return "", false
	}
	abs := filepath.Join(p.Root, filepath.FromSlash(rel))
	return abs, true
}

// IsInternal 判断是否属于 .hawk 内部（回收站除外）。
func IsInternal(rel string) bool {
	if strings.HasPrefix(rel, TrashPrefix) {
		return false
	}
	return rel == internalName || strings.HasPrefix(rel, internalName+"/")
}

// IsHidden 判断路径是否含以点开头的组件（回收站除外）。
func IsHidden(rel string) bool {
	if strings.HasPrefix(rel, TrashPrefix) {
		return false
	}
	for _, seg := range strings.Split(rel, "/") {
		if seg != "" && strings.HasPrefix(seg, ".") {
			return true
		}
	}
	return false
}

// IsInTrash 判断相对路径是否位于回收站。
func IsInTrash(rel string) bool { return strings.HasPrefix(rel, TrashPrefix) }

// TrashToLibraryPath 去掉回收站前缀，还原删除前的库内路径。
func TrashToLibraryPath(rel string) string { return strings.TrimPrefix(rel, TrashPrefix) }

// LibraryToTrashPath 把库内路径映射为回收站位置。
func LibraryToTrashPath(rel string) string { return TrashPrefix + rel }

// IsValidLibraryPath 校验 API 传入的库内相对路径：非空、不指向 .hawk 内部、不含越界成分。
func IsValidLibraryPath(rel string) bool {
	if rel == "" || strings.HasPrefix(rel, "/") || strings.Contains(rel, "\\") {
		return false
	}
	for _, seg := range strings.Split(rel, "/") {
		if seg == "" || seg == "." || seg == ".." {
			return false
		}
	}
	return !IsInternal(rel)
}

// DirOf 返回相对路径的父目录；根目录返回 ""。
func DirOf(rel string) string {
	if i := strings.LastIndexByte(rel, '/'); i >= 0 {
		return rel[:i]
	}
	return ""
}

// NameOf 返回文件名（不含扩展名）。以点开头的隐藏文件名整体保留。
func NameOf(rel string) string {
	base := rel
	if i := strings.LastIndexByte(rel, '/'); i >= 0 {
		base = rel[i+1:]
	}
	if i := strings.LastIndexByte(base, '.'); i > 0 {
		return base[:i]
	}
	return base
}

// ExtOf 返回小写扩展名（不含点）。
func ExtOf(rel string) string {
	base := rel
	if i := strings.LastIndexByte(rel, '/'); i >= 0 {
		base = rel[i+1:]
	}
	if i := strings.LastIndexByte(base, '.'); i > 0 {
		return strings.ToLower(base[i+1:])
	}
	return ""
}

// UnixMillis 转换为 Unix 毫秒。
func UnixMillis(t time.Time) int64 { return t.UnixNano() / int64(time.Millisecond) }

// FileMtimeMillis 返回文件修改时间（毫秒），失败返回 0。
func FileMtimeMillis(abs string) int64 {
	info, err := os.Stat(abs)
	if err != nil {
		return 0
	}
	return UnixMillis(info.ModTime())
}

// LibraryKey 返回库根路径 SHA-256 前 16 位小写十六进制。
func LibraryKey(root string) string {
	sum := sha256.Sum256([]byte(filepath.ToSlash(filepath.Clean(root))))
	return hex.EncodeToString(sum[:8])
}

func slash(p string) string { return filepath.ToSlash(p) }

func defaultCacheParent() string {
	switch runtime.GOOS {
	case "windows":
		if dir := os.Getenv("LOCALAPPDATA"); dir != "" {
			return filepath.Join(dir, "hawk", "cache")
		}
		if home := os.Getenv("USERPROFILE"); home != "" {
			return filepath.Join(home, "AppData", "Local", "hawk", "cache")
		}
	case "darwin":
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, "Library", "Application Support", "hawk", "cache")
		}
	default:
		if dir := os.Getenv("XDG_DATA_HOME"); dir != "" {
			return filepath.Join(dir, "hawk", "cache")
		}
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, ".local", "share", "hawk", "cache")
		}
	}
	return filepath.Join(os.TempDir(), "hawk", "cache")
}

func cacheDirName(root string) string {
	label := libraryLabel(root)
	return label + "_" + LibraryKey(root)
}

func libraryLabel(root string) string {
	trimmed := strings.TrimRight(slash(root), "/")
	name := path.Base(trimmed)
	if strings.TrimSpace(name) == "" || name == "." || name == "/" {
		return "library"
	}
	var b strings.Builder
	for _, r := range name {
		if r < 0x20 || strings.ContainsRune(`<>:"|?*`, r) {
			b.WriteRune('_')
		} else {
			b.WriteRune(r)
		}
	}
	cleaned := strings.TrimRight(b.String(), ". ")
	if cleaned == "" {
		return "library"
	}
	runes := []rune(cleaned)
	if len(runes) > 32 {
		runes = runes[:32]
	}
	return string(runes)
}

// SortEntries 按小写字典序排序（注册表统一顺序）。
func SortEntries(entries []string) {
	sort.Slice(entries, func(i, j int) bool {
		return strings.ToLower(entries[i]) < strings.ToLower(entries[j])
	})
}
