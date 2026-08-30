// 客户端图片编辑:解码/旋转/重编码全在渲染进程完成(canvas),server 只负责存储层内容替换(item/replace)。
// 职责边界见 docs/architecture.md:编辑计算归客户端,server 归存储与管理。
//
// 能力边界(canvas 编码器限制):
// - 仅支持 jpg/jpeg/png/webp,GIF(动图)/TIFF/BMP 等无法重编码,由 isRotatableImage 拦截
// - 单边长超过 MAX_CANVAS_SIDE 的图直接拒绝(canvas 必然编码失败)
//
// JPEG EXIF 保留:canvas 重编码会剥离全部元数据。旋转前取出原图的 APP1(EXIF)段,
// 将 Orientation 重置为 1(createImageBitmap 已把方向烘焙进像素,不重置会双重旋转),
// 编码完成后字节级插回 SOI 之后。任何一步解析失败都安全回退为「丢弃 EXIF」(浏览器默认行为)。
// 已知小瑕疵:EXIF 内嵌缩略图(IFD1)与尺寸标签在旋转后过期——基本无读取方,暂不重算。

export type RotateAngle = 90 | 180 | 270;

/** canvas 可重编码的扩展名白名单 */
const MIME_BY_EXT: Record<string, string> = {
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  png: 'image/png',
  webp: 'image/webp',
};

/** Chromium canvas 单边长上限(保守取值) */
const MAX_CANVAS_SIDE = 16384;

/** 顺时针角度 → canvas 旋转弧度(y 轴向下,正值为视觉上顺时针) */
const ROTATE_RADIAN: Record<RotateAngle, number> = {
  90: Math.PI / 2,
  180: Math.PI,
  270: -Math.PI / 2,
};

export function isRotatableImage(ext: string): boolean {
  return ext in MIME_BY_EXT;
}

// ---------- JPEG EXIF 保留(字节级,无第三方依赖) ----------

/**
 * 从 JPEG 字节中提取 APP1 Exif 段内容(含 "Exif\0\0" 头,不含段标记与长度字段)。
 * 非 JPEG、无 EXIF 或结构异常时返回 null。
 */
export function extractExifApp1(jpeg: Uint8Array): Uint8Array | null {
  if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8) {
    return null; // 应以 SOI 开头
  }

  let pos = 2;
  while (pos + 4 <= jpeg.length) {
    if (jpeg[pos] !== 0xff) {
      return null; // 头段结构异常
    }

    const marker = jpeg[pos + 1];
    if (marker === 0xda) {
      return null; // SOS:头段扫描结束,未见 EXIF
    }
    if (marker === 0xd8 || (marker >= 0xd0 && marker <= 0xd7)) {
      pos += 2; // SOI/Rn 独立标记,无长度字段
      continue;
    }

    const len = (jpeg[pos + 2] << 8) | jpeg[pos + 3];
    if (len < 2 || pos + 2 + len > jpeg.length) {
      return null; // 长度字段越界
    }
    // APP1 且内容为 Exif\0\0
    if (
      marker === 0xe1 &&
      len >= 8 &&
      jpeg[pos + 4] === 0x45 && // E
      jpeg[pos + 5] === 0x78 && // x
      jpeg[pos + 6] === 0x69 && // i
      jpeg[pos + 7] === 0x66 && // f
      jpeg[pos + 8] === 0 &&
      jpeg[pos + 9] === 0
    ) {
      return jpeg.slice(pos + 4, pos + 2 + len);
    }
    pos += 2 + len;
  }
  return null;
}

/**
 * 将 EXIF(TIFF)中的 Orientation(0x0112) 重置为 1。原地修改传入的段。
 * 返回是否可安全回填:true=已重置或无此标签;false=解析失败,调用方应丢弃 EXIF。
 */
export function resetExifOrientation(exif: Uint8Array): boolean {
  const tiff = exif.subarray(6); // 跳过 "Exif\0\0"
  if (tiff.length < 8) {
    return false;
  }

  const little = tiff[0] === 0x49 && tiff[1] === 0x49; // "II"
  const big = tiff[0] === 0x4d && tiff[1] === 0x4d; // "MM"
  if (!little && !big) {
    return false;
  }
  const u16 = (o: number) => (little ? tiff[o] | (tiff[o + 1] << 8) : (tiff[o] << 8) | tiff[o + 1]);
  const u32 = (o: number) =>
    little
      ? (tiff[o] | (tiff[o + 1] << 8) | (tiff[o + 2] << 16) | (tiff[o + 3] << 24)) >>> 0
      : ((tiff[o] << 24) | (tiff[o + 1] << 16) | (tiff[o + 2] << 8) | tiff[o + 3]) >>> 0;
  if (u16(2) !== 42) {
    return false; // TIFF 魔数
  }

  const ifd0 = u32(4);
  if (ifd0 + 2 > tiff.length) {
    return false;
  }
  const count = u16(ifd0);
  for (let i = 0; i < count; i++) {
    const entry = ifd0 + 2 + i * 12;
    if (entry + 12 > tiff.length) {
      return false;
    }
    if (u16(entry) === 0x0112) {
      // Orientation 类型必须为 SHORT 且数量为 1(值内联在条目前 2 字节)
      if (u16(entry + 2) !== 3 || u32(entry + 4) !== 1) {
        return false;
      }
      if (little) {
        tiff[entry + 8] = 1;
        tiff[entry + 9] = 0;
      } else {
        tiff[entry + 8] = 0;
        tiff[entry + 9] = 1;
      }
      return true;
    }
  }
  return true; // 无 Orientation 标签:不存在双重旋转风险,原样回填
}

/** 将 APP1 Exif 段插回 JPEG(SOI 之后);段总长超过 0xffff 时原样返回 */
export function insertExifApp1(jpeg: Uint8Array<ArrayBuffer>, exif: Uint8Array): Uint8Array<ArrayBuffer> {
  const segLen = exif.length + 2; // 长度字段含自身 2 字节
  if (jpeg.length < 2 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8 || segLen > 0xffff) {
    return jpeg;
  }

  const out = new Uint8Array(jpeg.length + 4 + exif.length);
  out.set(jpeg.subarray(0, 2), 0); // SOI
  out[2] = 0xff;
  out[3] = 0xe1; // APP1
  out[4] = segLen >> 8;
  out[5] = segLen & 0xff;
  out.set(exif, 6);
  out.set(jpeg.subarray(2), 6 + exif.length);
  return out;
}

/**
 * 旋转图像并重新编码,返回与原格式一致的 Blob。
 * createImageBitmap 默认按 EXIF 方向解码,方向信息烘焙进像素。
 */
export async function rotateImage(source: Blob, angle: RotateAngle, ext: string): Promise<Blob> {
  // JPEG 先取 EXIF 段,编码完成后回填;其余格式(canvas 产物本就无元数据)跳过
  const exif = ext === 'jpg' || ext === 'jpeg' ? extractExifApp1(new Uint8Array(await source.arrayBuffer())) : null;

  const bitmap = await createImageBitmap(source);
  try {
    const swapped = angle === 90 || angle === 270;
    const w = swapped ? bitmap.height : bitmap.width;
    const h = swapped ? bitmap.width : bitmap.height;
    if (w > MAX_CANVAS_SIDE || h > MAX_CANVAS_SIDE) {
      throw new Error(`图片过大(旋转后 ${w}×${h}),暂不支持旋转`);
    }

    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      throw new Error('无法创建绘图上下文');
    }

    ctx.translate(w / 2, h / 2);
    ctx.rotate(ROTATE_RADIAN[angle]);
    ctx.drawImage(bitmap, -bitmap.width / 2, -bitmap.height / 2);

    const mime = MIME_BY_EXT[ext] ?? 'image/png';
    // PNG 无损不传 quality;JPEG/WebP 固定 0.92(原图质量不可读,重编码一次代损)
    let blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, mime, mime === 'image/png' ? undefined : 0.92),
    );
    if (!blob) {
      throw new Error('图像编码失败');
    }
    // 回填 EXIF(Orientation 已重置);解析失败 resetExifOrientation 返回 false,保持丢弃语义
    if (exif && resetExifOrientation(exif)) {
      blob = new Blob([insertExifApp1(new Uint8Array(await blob.arrayBuffer()), exif)], { type: blob.type });
    }
    return blob;
  } finally {
    bitmap.close();
  }
}

/** Blob → 纯 Base64 字符串(去掉 data URL 前缀,直接作为 img_base64 提交) */
export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve((reader.result as string).split(',', 2)[1]);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}
