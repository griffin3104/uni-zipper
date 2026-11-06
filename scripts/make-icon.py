#!/usr/bin/env python3
"""アプリアイコン(assets/icon-1024.png)を作る。

使い方: python3 scripts/make-icon.py
その後 `npm run tauri icon assets/icon-1024.png` で、各OS用のアイコンを src-tauri/icons/ に生成する。

デザイン: 青い角丸の正方形に、白いジッパーの歯と、橙色のスライダー(Zipの色)。
画面のアクセント色(青)・Zipの色(橙)と合わせている。
"""
from PIL import Image, ImageChops, ImageDraw, ImageFilter

SIZE = 1024          # 出力
SS = 2               # 描画は2倍で行い、縮小して縁をなめらかにする
S = SIZE * SS


def px(v):
    return int(v * SS)


def gradient(box_size, top, bottom):
    """縦のグラデーション画像。"""
    w, h = box_size
    img = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(img)
    for y in range(h):
        t = y / (h - 1)
        color = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        d.line([(0, y), (w, y)], fill=color)
    return img


def rounded_mask(size, box, radius):
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=radius, fill=255)
    return mask


def main():
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # macOSのアイコンの作法: 1024のうち、824の角丸正方形を中央に置く(周りは透明)
    margin, body = 100, 824
    box = (px(margin), px(margin), px(margin + body), px(margin + body))
    radius = px(185)
    mask = rounded_mask((S, S), box, radius)

    # 影
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 90), (0, px(14)), mask)
    shadow = shadow.filter(ImageFilter.GaussianBlur(px(18)))
    canvas = Image.alpha_composite(canvas, shadow)

    # 本体(青のグラデーション)
    bg = gradient((S, S), (96, 142, 240), (40, 88, 208)).convert("RGBA")
    body_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    body_layer.paste(bg, (0, 0), mask)
    canvas = Image.alpha_composite(canvas, body_layer)

    # ジッパーは、本体の内側にだけ描く
    zip_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(zip_layer)
    cx = S // 2
    top = px(margin)
    tooth_w, tooth_h, pitch = px(84), px(46), px(60)
    gap = px(8)
    slider_top = px(500)

    # 歯: 左右が噛み合うように、交互にずらして並べる
    y = top + px(28)
    i = 0
    while y + tooth_h < slider_top:
        if i % 2 == 0:
            x0 = cx - tooth_w - gap // 2 + px(10)
        else:
            x0 = cx + gap // 2 - px(10)
        d.rounded_rectangle((x0, y, x0 + tooth_w, y + tooth_h), radius=px(14), fill=(255, 255, 255, 235))
        y += pitch
        i += 1

    # スライダー(橙): 本体 + 引き手
    orange_top, orange_bottom = (250, 178, 76), (232, 128, 36)
    sw, sh = px(150), px(190)
    slider_box = (cx - sw // 2, slider_top, cx + sw // 2, slider_top + sh)
    slider_mask = rounded_mask((S, S), slider_box, px(40))
    pull_box = (cx - px(54), slider_top + sh - px(46), cx + px(54), slider_top + sh + px(190))
    pull_mask = rounded_mask((S, S), pull_box, px(54))
    hole_box = (cx - px(22), slider_top + sh + px(60), cx + px(22), slider_top + sh + px(150))
    hole = rounded_mask((S, S), hole_box, px(22))
    shape = ImageChops.lighter(slider_mask, pull_mask)
    shape = ImageChops.subtract(shape, hole)
    grad = gradient((S, S), orange_top, orange_bottom).convert("RGBA")
    # スライダーの影(歯の上に少し落とす)
    sh_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sh_layer.paste((0, 0, 0, 70), (0, px(8)), shape)
    sh_layer = sh_layer.filter(ImageFilter.GaussianBlur(px(8)))
    zip_layer = Image.alpha_composite(zip_layer, sh_layer)
    zip_layer.paste(grad, (0, 0), shape)
    # スライダーの中央の溝(歯が通る道)
    d2 = ImageDraw.Draw(zip_layer)
    d2.rounded_rectangle((cx - px(14), slider_top + px(26), cx + px(14), slider_top + sh - px(30)), radius=px(14), fill=(190, 96, 14, 255))

    # 本体からはみ出した部分を切り落とす
    clipped = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clipped.paste(zip_layer, (0, 0), mask)
    canvas = Image.alpha_composite(canvas, clipped)

    out = canvas.resize((SIZE, SIZE), Image.LANCZOS)
    out.save("assets/icon-1024.png")
    print("assets/icon-1024.png を作りました")


if __name__ == "__main__":
    main()
