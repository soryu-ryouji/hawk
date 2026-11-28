package library

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func writePNG(t *testing.T, path string, c color.Color) {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 16, 12))
	for y := 0; y < 12; y++ {
		for x := 0; x < 16; x++ {
			img.Set(x, y, c)
		}
	}
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

func openTestLibrary(t *testing.T) *Library {
	t.Helper()
	dir := t.TempDir()
	root := filepath.Join(dir, "lib")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	lib, err := OpenLibrary(root, filepath.Join(dir, "cache"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(lib.Close)
	return lib
}

func TestPathsRelativeAndTrash(t *testing.T) {
	paths, err := NewPaths(filepath.Join(t.TempDir(), "lib"), t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := paths.EnsureLayout(); err != nil {
		t.Fatal(err)
	}
	rel, ok := paths.ToRelative(filepath.Join(paths.Root, "a", "b.png"))
	if !ok || rel != "a/b.png" {
		t.Fatalf("relative = %q, %v", rel, ok)
	}
	if abs, ok := paths.ToAbsolute(rel); !ok || abs != filepath.Join(paths.Root, "a", "b.png") {
		t.Fatalf("absolute = %q, %v", abs, ok)
	}
	if _, ok := paths.ToAbsolute("../escape.png"); ok {
		t.Fatal("越界路径不应通过")
	}
	if !IsInTrash(TrashPrefix+"a.png") || TrashToLibraryPath(TrashPrefix+"a.png") != "a.png" {
		t.Fatal("回收站路径换算错误")
	}
}

func TestIgnoreMatcherAndExtensionFilter(t *testing.T) {
	matcher := BuildIgnoreMatcher([]string{"node_modules", "*.tmp", "posters/2024"})
	for _, rel := range []string{"node_modules", "a/node_modules/x.js", "a/b/x.tmp", "posters/2024/cat.jpg"} {
		if !matcher.IsIgnored(rel) {
			t.Fatalf("应命中 ignore: %s", rel)
		}
	}
	if matcher.IsIgnored("posters/2025/cat.jpg") {
		t.Fatal("不应命中 ignore")
	}
	filter := BuildExtensionFilter([]string{".PNG", " jpg "})
	if !filter.Allows("a.png") || !filter.Allows("b.JPG") || filter.Allows("c.gif") {
		t.Fatal("扩展名白名单错误")
	}
}

func TestPaletteSolidColor(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 8, 8))
	for y := 0; y < 8; y++ {
		for x := 0; x < 8; x++ {
			img.Set(x, y, color.RGBA{R: 255, A: 255})
		}
	}
	palette := ExtractPalette(img)
	if len(palette) != 1 || palette[0].Color != "#ff0000" || palette[0].Percentage != 100 {
		t.Fatalf("palette = %+v", palette)
	}
}

func TestScanUpsertAndDelete(t *testing.T) {
	lib := openTestLibrary(t)
	writePNG(t, filepath.Join(lib.Paths.Root, "a.png"), color.RGBA{R: 255, A: 255})
	writePNG(t, filepath.Join(lib.Paths.Root, "b.png"), color.RGBA{B: 255, A: 255})
	time.Sleep(1100 * time.Millisecond)

	if err := lib.RunScan(scanOptions{}); err != nil {
		t.Fatal(err)
	}
	if got := lib.Index.Count(); got != 2 {
		t.Fatalf("count = %d", got)
	}
	result := lib.Index.Query(ItemQuery{})
	if result.Total != 2 {
		t.Fatalf("query total = %d", result.Total)
	}

	if err := os.Remove(filepath.Join(lib.Paths.Root, "a.png")); err != nil {
		t.Fatal(err)
	}
	if err := lib.RunScan(scanOptions{ForceWalk: true}); err != nil {
		t.Fatal(err)
	}
	if got := lib.Index.Count(); got != 1 {
		t.Fatalf("after delete count = %d", got)
	}
}

func TestMoveAndMetadataMigration(t *testing.T) {
	lib := openTestLibrary(t)
	root := lib.Paths.Root
	writePNG(t, filepath.Join(root, "a.png"), color.RGBA{G: 255, A: 255})
	time.Sleep(1100 * time.Millisecond)
	if err := lib.RunScan(scanOptions{}); err != nil {
		t.Fatal(err)
	}
	item := lib.Index.Query(ItemQuery{}).Items[0]
	if _, _, err := lib.ApplyMetadata(item.ID, func(current *Item) {
		current.Tags = []string{"keep"}
		current.Star = 3
	}); err != nil {
		t.Fatal(err)
	}
	newAbs := filepath.Join(root, "moved.png")
	if err := os.Rename(filepath.Join(root, "a.png"), newAbs); err != nil {
		t.Fatal(err)
	}
	lib.MovePath(filepath.Join(root, "a.png"), newAbs)
	dto, ok := lib.Index.GetDTO(item.ID)
	if !ok || dto.Path != "moved.png" || len(dto.Tags) != 1 || dto.Star != 3 {
		t.Fatalf("move result = %+v, %v", dto, ok)
	}
}

func TestLocksUnlockAndGuard(t *testing.T) {
	lib := openTestLibrary(t)
	if outcome := lib.Locks.Set(LockFolder, "private", "pw", nil); outcome != ManageOK {
		t.Fatalf("set outcome = %v", outcome)
	}
	guard := lib.Locks.ZeroGuard()
	if guard.EntryVisible("private/a.png", nil, nil) {
		t.Fatal("未解锁时应不可见")
	}
	outcome, ticket := lib.Locks.Unlock(LockFolder, "private", "pw")
	if outcome != UnlockGranted || ticket == "" {
		t.Fatalf("unlock = %v, %q", outcome, ticket)
	}
	guard = lib.Locks.Guard([]string{ticket})
	if !guard.EntryVisible("private/a.png", nil, nil) {
		t.Fatal("解锁后应可见")
	}
}
