# Camera library

Every file in here shows up in the library on the CAMERAS page of SWORTA. The app reads this folder straight from GitHub, so a merged file reaches everyone without a new release.

The folder a file sits in is its category: `action`, `mirrorless`, `cinema`, `drone`, `360` or `phone`. The file name has to match the `id` inside it.

## Adding a camera

The easiest way is to let SWORTA write the file. Plug in a card from the camera, click **Set up**, sort the file types the way they should land and save. Then **Edit** the camera, click **Export** and put the file into the right folder here with a pull request.

A camera file looks like this:

```json
{
  "id": "gopro-hero",
  "name": "GoPro HERO",
  "models": ["GoPro"],
  "markers": ["DCIM/*GOPRO"],
  "sources": ["DCIM"],
  "types": { "mp4": "VID", "jpg": "JPG", "gpr": "RAW", "lrv": "", "thm": "" },
  "folders": {},
  "layout": "{date}/{type}",
  "date_from_name": false
}
```

`models` is matched against make and model from the photos (`GoPro HERO12 Black`, `SONY ILCE-7M4`), a part is enough and the longest match wins. `markers` are paths only this camera creates on a card, `*` stands for any part of a folder name. `sources` are the card folders SWORTA reads. `types` maps a file extension to a folder at the destination, an empty value skips that type. `folders` does the same for everything inside one card folder, which is how Sony proxies in `PRIVATE/M4ROOT/SUB` end up apart from the clips. Targets can hold subfolders like `VID/PROXY`.

Before a pull request is merged, the tests make sure the file loads, the id is unique and a card with the camera's markers is recognised as that camera and not as another one. Run them with `cargo test`.
