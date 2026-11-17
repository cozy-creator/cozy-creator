Focus draws a picture's own file at its own pixels (cl-1184): never the 512 px thumbnail the canvas draws (a JPEG of an opaque picture), however small it's fitted. Its thumbnail shows at once until the file is decoded.
- Zoomed past one screen pixel to each of its own, a picture's pixels are drawn as they are, not smoothed, as an image editor draws them.
- A picture too large to reach its own pixels at 8× its fit (a 16,384 px panorama) zooms to 8 screen pixels to each of them.
