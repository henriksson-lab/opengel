## The `.gel.zip` format

A ZIP container:

```
manifest.json   { format, version, gel_type }
metadata.json   [ per-image capture params: exposure, gain, camera, bracket ]
analysis.json   lanes, bands, blobs, ladder assignments, calibration, quant
images/img_NN.png   raw captures (8- or 16-bit)
```
