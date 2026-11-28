package library

import (
	"strings"
)

// Role 是固定权限角色。admin 只由本机进程管理员持有。
type Role string

const (
	RoleViewer   Role = "viewer"
	RoleImporter Role = "importer"
	RoleEditor   Role = "editor"
	RoleAdmin    Role = "admin"
)

func (r Role) Valid() bool {
	switch r {
	case RoleViewer, RoleImporter, RoleEditor, RoleAdmin:
		return true
	}
	return false
}

// Channel 是请求穿过的信任边界，由监听器写入，客户端不能伪造。
type Channel string

const (
	ChannelLoopback Channel = "loopback"
	ChannelLanWeb   Channel = "lan_web"
	ChannelLanPeer  Channel = "lan_peer"
)

// Capability 表示一类操作能力。
type Capability uint8

const (
	CapRead Capability = 1 << iota
	CapImport
	CapEdit
	CapManage
)

func (c Capability) String() string {
	switch c {
	case CapRead:
		return "read"
	case CapImport:
		return "import"
	case CapEdit:
		return "edit"
	case CapManage:
		return "manage"
	}
	return "unknown"
}

// PaletteEntry 是 SQLite/API 的调色板条目。
type PaletteEntry struct {
	Color      string  `json:"color"`
	Percentage float64 `json:"percentage"`
}

// PathEntry 是一个文件位置。Path 为库内相对路径（回收站位置以 .hawk/trash/ 开头）。
type PathEntry struct {
	Path             string
	Size             int64
	ModificationTime int64
}

// PaletteColor 是索引中的调色板颜色，附带预算的 Lab 检索坐标。
type PaletteColor struct {
	R          uint8
	G          uint8
	B          uint8
	Percentage float64
	Lab        LabColor
}

// Item 是内存索引中的一条素材：内容 hash + 全部位置 + 查询用元数据副本。
type Item struct {
	ID        string
	Locations []PathEntry

	URL        string
	Tags       []string
	Categories []string
	Star       int
	Annotation string

	Width  int
	Height int
	// Palette nil = 未提炼；非 nil 空切片 = 已确认非图像（负缓存）
	Palette    []PaletteEntry
	PaletteSet bool

	paletteLab []PaletteColor
}

func newItem(id string) *Item {
	return &Item{
		ID:         id,
		Locations:  []PathEntry{},
		Tags:       []string{},
		Categories: []string{},
		Palette:    []PaletteEntry{},
	}
}

func (it *Item) hasLibraryLocations() bool {
	for _, loc := range it.Locations {
		if !IsInTrash(loc.Path) {
			return true
		}
	}
	return false
}

func (it *Item) mainLocation(trashView bool) *PathEntry {
	for i := range it.Locations {
		if IsInTrash(it.Locations[i].Path) == trashView {
			return &it.Locations[i]
		}
	}
	return nil
}

func (it *Item) locationAt(rel string, trashView bool) *PathEntry {
	for i := range it.Locations {
		loc := &it.Locations[i]
		if IsInTrash(loc.Path) == trashView && (loc.Path == rel || TrashToLibraryPath(loc.Path) == rel) {
			return loc
		}
	}
	return nil
}

func (it *Item) rebuildPaletteLab() {
	it.paletteLab = it.paletteLab[:0]
	for _, entry := range it.Palette {
		if r, g, b, ok := ParseHex(entry.Color); ok {
			it.paletteLab = append(it.paletteLab, PaletteColor{
				R:          r,
				G:          g,
				B:          b,
				Percentage: entry.Percentage,
				Lab:        RGBToLab(r, g, b),
			})
		}
	}
}

// PaletteColorDTO 是 API 的调色板颜色项。
type PaletteColorDTO struct {
	Color      string  `json:"color"`
	Percentage float64 `json:"percentage"`
}

// ItemDTO 是 API 的 Item 对象。
type ItemDTO struct {
	ID               string            `json:"id"`
	Path             string            `json:"path"`
	Name             string            `json:"name"`
	Ext              string            `json:"ext"`
	Width            int               `json:"width"`
	Height           int               `json:"height"`
	Size             int64             `json:"size"`
	URL              *string           `json:"url,omitempty"`
	Tags             []string          `json:"tags"`
	Categories       []string          `json:"categories"`
	Paths            []string          `json:"paths"`
	Folders          []string          `json:"folders"`
	Star             int               `json:"star"`
	Annotation       *string           `json:"annotation,omitempty"`
	ModificationTime int64             `json:"modification_time"`
	Palette          []PaletteColorDTO `json:"palette"`
}

// ItemSkeletonDTO 是网格虚拟布局所需的最低限度信息。
type ItemSkeletonDTO struct {
	ID     string `json:"id"`
	Path   string `json:"path"`
	Width  int    `json:"width"`
	Height int    `json:"height"`
	Star   int    `json:"star"`
	Size   int64  `json:"size"`
}

func (it *Item) toDTOAt(loc *PathEntry, trashView bool) ItemDTO {
	paths := make([]string, 0, len(it.Locations))
	folders := make([]string, 0, len(it.Locations))
	seen := make(map[string]struct{}, len(it.Locations))
	for i := range it.Locations {
		l := &it.Locations[i]
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

	palette := make([]PaletteColorDTO, 0, len(it.Palette))
	for _, entry := range it.Palette {
		palette = append(palette, PaletteColorDTO{Color: entry.Color, Percentage: entry.Percentage})
	}

	display := loc.Path
	if trashView {
		display = TrashToLibraryPath(loc.Path)
	}
	var url *string
	if it.URL != "" {
		u := it.URL
		url = &u
	}
	var annotation *string
	if it.Annotation != "" {
		a := it.Annotation
		annotation = &a
	}

	return ItemDTO{
		ID:               it.ID,
		Path:             loc.Path,
		Name:             NameOf(display),
		Ext:              ExtOf(display),
		Width:            it.Width,
		Height:           it.Height,
		Size:             loc.Size,
		URL:              url,
		Tags:             append([]string{}, it.Tags...),
		Categories:       append([]string{}, it.Categories...),
		Paths:            paths,
		Folders:          folders,
		Star:             it.Star,
		Annotation:       annotation,
		ModificationTime: loc.ModificationTime,
		Palette:          palette,
	}
}

func (it *Item) toDTO(trashView bool) (ItemDTO, bool) {
	loc := it.mainLocation(trashView)
	if loc == nil {
		return ItemDTO{}, false
	}
	return it.toDTOAt(loc, trashView), true
}

// ItemQuery 是 item/list 与 item/skeleton 的查询条件（AND 语义）。
type ItemQuery struct {
	IDs               []string
	Keywords          []string
	Tags              []string
	Star              *int
	Folders           []string
	FoldersExact      bool
	Categories        []string
	CategoriesMatch   string
	ExcludeCategories []string
	ExcludeTags       []string
	ExcludeFolders    []string
	WithoutCategories bool
	WithoutTags       bool
	Ext               string
	Annotation        string
	URL               string
	MinSideGte        *int
	MinSideLte        *int
	MinWidth          *int
	MaxWidth          *int
	MinHeight         *int
	MaxHeight         *int
	Color             *LabColor
	LockGuard         LockGuard
	InTrash           bool
	OrderBy           string
	Order             string
	Offset            int
	Limit             int
}

func containsString(list []string, value string) bool {
	for _, item := range list {
		if item == value {
			return true
		}
	}
	return false
}

func containsFold(list []string, value string) bool {
	lower := strings.ToLower(value)
	for _, item := range list {
		if strings.ToLower(item) == lower {
			return true
		}
	}
	return false
}
