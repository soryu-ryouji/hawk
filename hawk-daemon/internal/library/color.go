package library

import (
	"fmt"
	"image"
	"math"
	"sort"
	"strconv"
	"strings"

	xdraw "golang.org/x/image/draw"
)

// LabColor 是 CIELAB 颜色。
type LabColor struct {
	L float64
	A float64
	B float64
}

// ParseHex 解析 "#344441" 或 "344441"；非法返回 false。
func ParseHex(text string) (uint8, uint8, uint8, bool) {
	s := strings.TrimSpace(text)
	s = strings.TrimPrefix(s, "#")
	if len(s) != 6 {
		return 0, 0, 0, false
	}
	v, err := strconv.ParseUint(s, 16, 32)
	if err != nil {
		return 0, 0, 0, false
	}
	return uint8(v >> 16), uint8(v >> 8), uint8(v), true
}

// ToHex 格式化为小写 #rrggbb。
func ToHex(r, g, b uint8) string {
	return fmt.Sprintf("#%02x%02x%02x", r, g, b)
}

// RGBToLab 把 sRGB 转换为 CIELAB（D65）。
func RGBToLab(r, g, b uint8) LabColor {
	rl := pivotRGB(float64(r) / 255.0)
	gl := pivotRGB(float64(g) / 255.0)
	bl := pivotRGB(float64(b) / 255.0)

	x := (rl*0.4124564 + gl*0.3575761 + bl*0.1804375) / 0.95047
	y := rl*0.2126729 + gl*0.7151522 + bl*0.0721750
	z := (rl*0.0193339 + gl*0.1191920 + bl*0.9503041) / 1.08883

	fx := pivotXYZ(x)
	fy := pivotXYZ(y)
	fz := pivotXYZ(z)

	return LabColor{
		L: 116.0*fy - 16.0,
		A: 500.0 * (fx - fy),
		B: 200.0 * (fy - fz),
	}
}

// DeltaESquared 返回 CIE76 ΔE 的平方。
func DeltaESquared(a, b LabColor) float64 {
	dl := a.L - b.L
	da := a.A - b.A
	db := a.B - b.B
	return dl*dl + da*da + db*db
}

func pivotRGB(c float64) float64 {
	if c <= 0.04045 {
		return c / 12.92
	}
	return math.Pow((c+0.055)/1.055, 2.4)
}

func pivotXYZ(t float64) float64 {
	if t > 0.008856 {
		return math.Cbrt(t)
	}
	return 7.787*t + 16.0/116.0
}

const (
	paletteSize    = 10
	analysisSize   = 64
	alphaThreshold = 128
)

// ExtractPalette 从已解码图像提炼调色板。无有效像素返回空切片（负缓存）。
func ExtractPalette(src image.Image) []PaletteEntry {
	b := src.Bounds()
	w, h := b.Dx(), b.Dy()
	if w <= 0 || h <= 0 {
		return []PaletteEntry{}
	}
	scale := math.Min(float64(analysisSize)/float64(w), float64(analysisSize)/float64(h))
	if scale > 1 {
		scale = 1
	}
	dw := max(1, int(math.Round(float64(w)*scale)))
	dh := max(1, int(math.Round(float64(h)*scale)))
	small := image.NewRGBA(image.Rect(0, 0, dw, dh))
	xdraw.CatmullRom.Scale(small, small.Bounds(), src, b, xdraw.Over, nil)

	pixels := make([][3]uint8, 0, dw*dh)
	for y := 0; y < dh; y++ {
		row := small.Pix[y*small.Stride:]
		for x := 0; x < dw; x++ {
			alpha := row[x*4+3]
			if alpha < alphaThreshold {
				continue
			}
			pixels = append(pixels, [3]uint8{row[x*4], row[x*4+1], row[x*4+2]})
		}
	}
	if len(pixels) == 0 {
		return []PaletteEntry{}
	}
	return medianCut(pixels)
}

func medianCut(pixels [][3]uint8) []PaletteEntry {
	total := len(pixels)
	boxes := [][]int{makeIndexes(len(pixels))}

	for len(boxes) < paletteSize {
		best := -1
		bestRange := 0
		bestChannel := 0
		for i, box := range boxes {
			if len(box) < 2 {
				continue
			}
			for ch := 0; ch < 3; ch++ {
				minV, maxV := uint8(255), uint8(0)
				for _, idx := range box {
					v := pixels[idx][ch]
					if v < minV {
						minV = v
					}
					if v > maxV {
						maxV = v
					}
				}
				if r := int(maxV - minV); r > bestRange {
					best = i
					bestRange = r
					bestChannel = ch
				}
			}
		}
		if best < 0 {
			break
		}
		box := boxes[best]
		boxes = append(boxes[:best], boxes[best+1:]...)
		sort.Slice(box, func(i, j int) bool {
			a, b := pixels[box[i]], pixels[box[j]]
			if a[bestChannel] != b[bestChannel] {
				return a[bestChannel] < b[bestChannel]
			}
			if a[(bestChannel+1)%3] != b[(bestChannel+1)%3] {
				return a[(bestChannel+1)%3] < b[(bestChannel+1)%3]
			}
			return a[(bestChannel+2)%3] < b[(bestChannel+2)%3]
		})
		mid := len(box) / 2
		left := append([]int{}, box[:mid]...)
		right := append([]int{}, box[mid:]...)
		boxes = append(boxes, left, right)
	}

	out := make([]PaletteEntry, 0, len(boxes))
	for _, box := range boxes {
		if len(box) == 0 {
			continue
		}
		var rs, gs, bs uint64
		for _, idx := range box {
			rs += uint64(pixels[idx][0])
			gs += uint64(pixels[idx][1])
			bs += uint64(pixels[idx][2])
		}
		n := uint64(len(box))
		percentage := math.Round(float64(len(box))*1000.0/float64(total)) / 10.0
		out = append(out, PaletteEntry{
			Color:      ToHex(uint8(rs/n), uint8(gs/n), uint8(bs/n)),
			Percentage: percentage,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Percentage != out[j].Percentage {
			return out[i].Percentage > out[j].Percentage
		}
		return out[i].Color < out[j].Color
	})
	if len(out) > paletteSize {
		out = out[:paletteSize]
	}
	return out
}

func makeIndexes(n int) []int {
	out := make([]int, n)
	for i := range out {
		out[i] = i
	}
	return out
}
