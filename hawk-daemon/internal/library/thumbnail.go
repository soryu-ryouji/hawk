package library

import (
	"bytes"
	"fmt"
	"image"
	"io"
	"os"
	"path/filepath"
	"strconv"

	"github.com/disintegration/imaging"
	"github.com/gen2brain/webp"

	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"

	_ "golang.org/x/image/bmp"
	_ "golang.org/x/image/tiff"
)

const thumbnailQuality = 80

var thumbnailSizes = []int{512, 1024}

type deriveJob struct {
	Hash  string
	Force bool
}

func (l *Library) startDeriveWorkers() {
	workers := l.scanWorkers()
	if workers > 8 {
		workers = 8
	}
	for i := 0; i < workers; i++ {
		l.deriveWG.Add(1)
		go func() {
			defer l.deriveWG.Done()
			for {
				select {
				case <-l.deriveStop:
					return
				case job := <-l.deriveCh:
					l.deriveActive.Add(1)
					l.PublishTaskProgress("thumbnail", "", 0, 0)
					l.runDerive(job)
					l.deriveActive.Add(-1)
					l.inflightMu.Lock()
					delete(l.inflight, job.Hash)
					l.inflightMu.Unlock()
					l.PublishTaskProgress("thumbnail", "", 0, 0)
				}
			}
		}()
	}
}

func (l *Library) enqueueDerive(hash string, force bool) {
	l.inflightMu.Lock()
	if _, exists := l.inflight[hash]; exists && !force {
		l.inflightMu.Unlock()
		return
	}
	l.inflight[hash] = struct{}{}
	l.inflightMu.Unlock()
	select {
	case l.deriveCh <- deriveJob{Hash: hash, Force: force}:
		l.PublishTaskProgress("thumbnail", "", 0, 0)
	default:
		l.inflightMu.Lock()
		delete(l.inflight, hash)
		l.inflightMu.Unlock()
	}
}

func (l *Library) runDerive(job deriveJob) {
	abs, ok := l.Index.MainSourceAbs(job.Hash, l.Paths)
	if !ok {
		return
	}
	ext := ExtOf(abs)
	if !isImageExtension(ext) {
		l.applyDerived(job.Hash, 0, 0, []PaletteEntry{}, true)
		return
	}
	img, err := decodeImage(abs)
	if err != nil {
		// 图片解码失败保持未提炼状态，读取端/维护会重试。
		return
	}
	bounds := img.Bounds()
	width, height := bounds.Dx(), bounds.Dy()
	palette := ExtractPalette(img)
	if err := l.generateThumbnails(img, job.Hash, job.Force); err != nil {
		fmt.Fprintf(os.Stderr, "缩略图生成失败 %s: %v\n", job.Hash, err)
	}
	l.applyDerived(job.Hash, width, height, palette, true)
}

func (l *Library) applyDerived(hash string, width, height int, palette []PaletteEntry, paletteSet bool) {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()
	item, ok := l.Index.ItemSnapshot(hash)
	if !ok {
		return
	}
	item.Width = width
	item.Height = height
	if paletteSet {
		item.Palette = palette
		item.PaletteSet = true
		item.rebuildPaletteLab()
	}
	l.Index.UpdateItem(hash, func(current *Item) {
		current.Width = width
		current.Height = height
		if paletteSet {
			current.Palette = append([]PaletteEntry{}, palette...)
			current.PaletteSet = true
			current.rebuildPaletteLab()
		}
	})
	if err := l.DB.Save(item); err != nil {
		fmt.Fprintf(os.Stderr, "派生元数据写入失败 %s: %v\n", hash, err)
		return
	}
	l.publishChanged(hash)
}

func isImageExtension(ext string) bool {
	switch ext {
	case "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff":
		return true
	}
	return false
}

func decodeImage(abs string) (image.Image, error) {
	data, err := os.ReadFile(abs)
	if err != nil {
		return nil, err
	}
	if img, _, err := image.Decode(bytes.NewReader(data)); err == nil {
		return img, nil
	}
	return webp.Decode(bytes.NewReader(data))
}

func (l *Library) thumbnailPath(hash string, size int) string {
	return filepath.Join(l.Paths.ThumbnailsDir, strconv.Itoa(size), hash+".webp")
}

// ThumbnailFile 返回已缓存的缩略图路径；未命中时后台入队生成。
func (l *Library) ThumbnailFile(hash string, size int) (string, bool) {
	path := l.thumbnailPath(hash, size)
	if info, err := os.Stat(path); err == nil && info.Size() > 0 {
		return path, true
	}
	l.enqueueDerive(hash, false)
	return "", false
}

// RefreshThumbnail 强制重建缩略图并补齐调色板/宽高。
func (l *Library) RefreshThumbnail(hash string) {
	l.enqueueDerive(hash, true)
}

func (l *Library) generateThumbnails(img image.Image, hash string, force bool) error {
	for _, size := range thumbnailSizes {
		path := l.thumbnailPath(hash, size)
		if !force {
			if info, err := os.Stat(path); err == nil && info.Size() > 0 {
				continue
			}
		}
		resized := resizeToFit(img, size)
		var buf bytes.Buffer
		if err := webp.Encode(&buf, resized, webp.Options{Quality: thumbnailQuality, Method: 4}); err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		tmp := path + ".tmp"
		if err := os.WriteFile(tmp, buf.Bytes(), 0o644); err != nil {
			return err
		}
		if err := os.Rename(tmp, path); err != nil {
			_ = os.Remove(tmp)
			return err
		}
	}
	return nil
}

func resizeToFit(img image.Image, size int) image.Image {
	bounds := img.Bounds()
	w, h := bounds.Dx(), bounds.Dy()
	if w <= size && h <= size {
		return img
	}
	return imaging.Fit(img, size, size, imaging.Lanczos)
}

func (l *Library) deleteThumbnails(hash string) {
	for _, size := range thumbnailSizes {
		_ = os.Remove(l.thumbnailPath(hash, size))
	}
}

func (l *Library) runDerivedMaintenance() {
	hashes := l.Index.HashesInScope(RefreshScope{Kind: "library"})
	for _, hash := range hashes {
		item, ok := l.Index.ItemSnapshot(hash)
		if !ok {
			continue
		}
		if item.Width == 0 || !item.PaletteSet {
			l.enqueueDerive(hash, false)
		}
	}
}

// ThumbnailStats 返回缩略图队列快照。
func (l *Library) ThumbnailStats() ThumbnailStats {
	pending := len(l.deriveCh)
	active := int(l.deriveActive.Load())
	return ThumbnailStats{Pending: pending, Active: active}
}

// thumbnailBytes 读取已有缩略图内容（测试/诊断用）。
func (l *Library) thumbnailBytes(hash string, size int) ([]byte, bool) {
	path, ok := l.ThumbnailFile(hash, size)
	if !ok {
		return nil, false
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, false
	}
	defer f.Close()
	data, err := io.ReadAll(f)
	return data, err == nil
}
