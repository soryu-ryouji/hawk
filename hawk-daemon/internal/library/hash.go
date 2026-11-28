package library

import (
	"bytes"
	"encoding/hex"
	"image"
	"io"
	"os"
	"time"

	"github.com/gen2brain/webp"
	"github.com/zeebo/blake3"
)

// HashFile 返回文件内容的 BLAKE3 hex。
func HashFile(abs string) (string, error) { return hashFile(abs) }

// HashBytes 返回内存内容的 BLAKE3 hex。
func HashBytes(data []byte) string {
	hasher := blake3.New()
	_, _ = hasher.Write(data)
	return hex.EncodeToString(hasher.Sum(nil))
}

// HashReader 流式计算 BLAKE3。
func HashReader(reader io.Reader) (string, error) {
	hasher := blake3.New()
	buf := make([]byte, 1024*1024)
	if _, err := io.CopyBuffer(hasher, reader, buf); err != nil {
		return "", err
	}
	return hex.EncodeToString(hasher.Sum(nil)), nil
}

// DetectImageFormat 返回 image.DecodeConfig 识别的格式名（jpeg/png/gif/webp...）。
func DetectImageFormat(data []byte) (string, image.Config, error) {
	cfg, format, err := image.DecodeConfig(bytes.NewReader(data))
	if err == nil {
		return format, cfg, nil
	}
	if webpCfg, webpErr := webp.DecodeConfig(bytes.NewReader(data)); webpErr == nil {
		return "webp", webpCfg, nil
	}
	return "", image.Config{}, err
}

// SetFileTimes 把文件访问/修改时间设置为给定毫秒时间戳。
func SetFileTimes(abs string, mtimeMS int64) {
	info, err := os.Stat(abs)
	if err != nil {
		return
	}
	t := info.ModTime()
	if mtimeMS > 0 {
		t = time.UnixMilli(mtimeMS)
	}
	_ = os.Chtimes(abs, t, t)
}
