# Enhanced HD resources

`opening-menu-720p.png` is a 1280×720 ImageGen enhancement of the original DOS
opening-menu background. The generated image intentionally contains no menu
labels: rustpal composites its live indexed-color text over the HD background,
so selection colors and input behavior remain native to the engine.

The source screenshot was used as an edit target with these constraints:

- preserve the gourds, bamboo slips, diagonal sword, black backdrop, and their
  relative composition;
- remove only the baked-in Chinese menu labels;
- enhance material detail and lighting in the original Chinese-fantasy style;
- extend the canvas to 16:9 without adding text, logos, watermarks, UI, borders,
  or new objects.

The generated result was Lanczos-resampled and center-cropped to exactly
1280×720 before being embedded in the runtime.
