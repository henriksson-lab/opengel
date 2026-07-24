# Commercial DNA / RNA / Protein Ladders

Reference catalog of the most commonly used molecular-weight ladders, compiled
from vendor datasheets and product pages. The machine-readable version (used by
the app's ladder dropdown and the detection engine) lives in
`src/core/ladders/ladders.json`.

Legend: **bold** = reference ("extra-thick", increased-intensity) band. Sizes in
bp (DNA), nt (RNA), or kDa (protein). Per-band ng is listed where the vendor
publishes it (mostly the DNA/RNA quantitation ladders; prestained protein
standards publish protein concentration, not per-band ng).

Provenance note: NEB DNA/ssRNA per-band masses are from NEB datasheets
(N3232, N3231, N0362). Thermo DNA per-band ng appears only as labels on each
product-page gel image and is not transcribed here (only the ~0.5 µg total load
is given in text); RNA ng is from the RiboRuler guides. Protein reference-band
colors/intensities are from vendor pages. Values not independently confirmable
to the band level are omitted rather than guessed.

---

## New England Biolabs (NEB)

### 1 kb DNA Ladder — N3232 (DNA)
https://www.neb.com/en-us/products/n3232-1-kb-dna-ladder — load 0.5 µg/lane.
10, 8, 6, 5, 4, **3**, 2, 1.5, 1, 0.5 kb.
ng/band: 42, 42, 50, 42, 33, **125**, 48, 36, 42, 42.

### 1 kb Plus DNA Ladder — N3200 (DNA)  *(formerly 2-Log DNA Ladder)*
https://www.neb.com/en-us/products/n3200-1-kb-plus-dna-ladder
10000, 8000, 6000, 5000, 4000, **3000**, 2000, 1500, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 100 bp DNA Ladder — N3231 (DNA)
https://www.neb.com/en-us/products/n3231-100-bp-dna-ladder — load 0.5 µg/lane.
1517, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.
ng/band: 45, 35, **95**, 27, 24, 21, 18, **97**, 38, 29, 25, 48.

### ssRNA Ladder — N0362 (RNA)
https://www.neb.com/en-us/products/n0362-ssrna-ladder — load 1 µg (native) / 2–3 µg (denaturing).
9000, 7000, 5000, **3000**, 2000, 1000, 500 bases. (Low Range ssRNA Ladder: N0364.)

### Color Prestained Protein Standard, Broad Range — P7719 (protein)
https://www.neb.com/en-us/products/p7719-color-prestained-protein-standard-broad-range-10-250-kda
250, 180, 130, 95, **72 (orange)**, 55, 43, 34, **26 (green)**, 17, 11 kDa.

### Blue Prestained Protein Standard, Broad Range — P7718 (protein)
https://www.neb.com/en-us/products/p7718-blue-prestained-protein-standard-broad-range-11-250-kda
250, 180, 130, 95, 72, 55, 43, 34, 26, 17, 11 kDa.

### Unstained Protein Standard, Broad Range — P7717 (protein)
https://www.neb.com/en-us/products/p7717-unstained-protein-standard-broad-range-10-200-kda
200, 150, 120, 100, 85, 70, 60, 50, 40, 30, 25, 15, 10 kDa.

---

## Thermo Fisher Scientific (Thermo Scientific / Invitrogen)

DNA GeneRuler/Invitrogen: stock 0.5 µg/µL, standard load ~0.5 µg (~0.3 µg for High Range).

### GeneRuler 1 kb — SM0311 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0311
10000, 8000, **6000**, 5000, 4000, 3500, **3000**, 2500, 2000, 1500, **1000**, 750, 500, 250 bp.

### GeneRuler 1 kb Plus — SM1331 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1331
20000, 10000, 7000, **5000**, 4000, 3000, 2000, **1500**, 1000, 700, **500**, 400, 300, 200, 75 bp.

### GeneRuler 100 bp — SM0241 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0241
1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### GeneRuler 100 bp Plus — SM0321 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0321
3000, 2000, 1500, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### GeneRuler Low Range — SM1191 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1191
700, 500, 400, **300**, 200, 150, **100**, 75, 50, 25 bp.

### GeneRuler High Range — SM1351 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1351
48502, 24508, 20555, 17000, 15258, 13825, 12119, 10171 bp.

### GeneRuler Ultra Low Range — SM1211 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1211
300, 200, 150, 100, 75, **50**, 35, 25, 20, 15, 10 bp.

### Invitrogen 1 Kb Plus DNA Ladder — 10787018 (DNA)
https://www.thermofisher.com/order/catalog/product/10787018
15000, 10000, 8000, 7000, 6000, 5000, 4000, 3000, 2000, **1500**, 1000, 850, 650, 500, 400, 300, 200, 100 bp.

### RiboRuler High Range RNA — SM1821 (RNA)
https://www.thermofisher.com/order/catalog/product/SM1821 — 120 ng/band (~960 ng total).
6000, 4000, 3000, 2000, 1500, 1000, 500, 200 nt.

### RiboRuler Low Range RNA — SM1831 (RNA)
https://www.thermofisher.com/order/catalog/product/SM1831 — 140 ng/band (~980 ng total).
1000, 800, 600, 400, 300, 200, 100 nt.

### Millennium RNA Markers — AM7150 (RNA)
https://www.thermofisher.com/order/catalog/product/AM7150
9000, 6000, 5000, 4000, 3000, 2500, 2000, 1500, 1000, 500 nt.

### PageRuler Prestained (10–180 kDa) — 26616 (protein)
https://www.thermofisher.com/order/catalog/product/26616
180, 130, 100, **70 (orange)**, 55, 40, 35, 25, 15, **10 (green)** kDa.

### PageRuler Plus Prestained (10–250 kDa) — 26619 (protein)
https://www.thermofisher.com/order/catalog/product/26619
250, 130, 100, 70, 55, 35, 25, 15, 10 kDa.

### PageRuler Unstained (10–200 kDa) — 26614 (protein)
https://www.thermofisher.com/order/catalog/product/26614
200, 150, 120, 100, 85, 70, 60, **50**, 40, 30, 25, 20, 15, 10 kDa.

### PageRuler Unstained Broad Range (5–250 kDa) — 26630 (protein)
https://www.thermofisher.com/order/catalog/product/26630
250, 150, **100**, 70, **50**, 40, 30, **20**, 15, 10, 5 kDa.

### Spectra Multicolor Broad Range (10–260 kDa) — 26623 (protein)
https://www.thermofisher.com/order/catalog/product/26623
260, 140, 100, 70, 50, 40, 35, 25, 15, 10 kDa.

---

## Bio-Rad

### Precision Plus Protein All Blue — 1610373 (protein)
https://www.bio-rad.com/en-us/sku/1610373 — 250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Precision Plus Protein Dual Color — 1610374 (protein)
https://www.bio-rad.com/en-us/sku/1610374 — 250, 150, 100, 75 (pink), 50, 37, 25 (pink), 20, 15, 10 kDa.

### Precision Plus Protein Dual Xtra — 1610377 (protein)
https://www.bio-rad.com/en-us/sku/1610377 — 250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10, 5, **2** kDa.

### EZ Load 100 bp Molecular Ruler — 1708352 (DNA)
https://www.bio-rad.com/en-us/sku/1708352 — 1000, 900, 800, 700, 600, 500, 400, 300, **200**, 100 bp.

### EZ Load Precision Molecular Mass Ruler — 1708356 (DNA, quantitation)
https://www.bio-rad.com/en-us/sku/1708356 — total 250 ng.
1000 = 100 ng, 700 = 70 ng, 500 = 50 ng, 200 = 20 ng, 100 = 10 ng.

*(Bio-Rad does not market a common RNA size ladder.)*

---

## Promega

### 1 kb DNA Ladder — G5711 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/1kb-dna-ladder/
10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1000**, 750, 500, 250 bp.

### 100 bp DNA Ladder — G2101 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/100bp-dna-ladder/
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### PCR Markers — G3161 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/pcr-markers/
1000, 750, 500, 300, 150, 50 bp (equal intensity).

### λ DNA/HindIII Markers — G1711 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/lambda-dna-hindiii-markers/
23130, 9416, 6557, 4361, 2322, 2027, 564, 125 bp.

### RNA Markers — G3191 (RNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/rna-markers/
6583, 4981, 3638, 2604, 1908, 1383, 955, 623, 281 nt.

### Broad Range Protein Molecular Weight Markers — V8491 (protein)
https://www.promega.com/products/protein-analysis/protein-molecular-weight-markers/broad-range-protein-molecular-weight-markers/
225, 150, 100, 75, **50**, 35, 25, 15, 10 kDa (50 kDa loaded at 3×).

---

## Takara Bio

### 20 bp DNA Ladder — 3409A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
**500**, 400, 300, **200**, 180, 160, 140, 120, **100**, 80, 60, 40, 20 bp.

### 100 bp DNA Ladder — 3407A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 200 bp DNA Ladder — 3410A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
4000, 3000, 2500, **2000**, 1800, 1600, 1400, 1200, **1000**, 800, 600, 400, 200 bp.

## GoldBio

### ReadyLadder 50 bp-25 kb DNA Ladder — D015 (DNA)
https://www.goldbio.com/products/superladder-50-bp-25-kb
25000, 10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1200**, 1000, 900, 800, 700, 600, **500**, 450, 400, 350, 300, 250, **200**, 150, 100, 50 bp.

## GenScript

### Broad Multi Color Pre-Stained Protein Standard — M00624S (protein)
https://www.genscript.com/molecule/M00624S-Broad_Multi_Color_Pre_Stained_Protein_Standard.html
**270**, 175, 130, 95, 65, **50**, 35, **30**, 15, 5 kDa.

## Abcam

### Prestained Protein Ladder, Broad Range — ab116028 (protein)
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-broad-molecular-weight-10-245-kda-ab116028
245, 180, 135, 100, **75**, 63, 48, 35, **25**, 20, 17, 10 kDa.

Abcam's product page publishes the range, band count, and 75/25 kDa reference
bands in text. The exact full 12-band list is from distributor-provided Prism
Ultra/ab116028 product text and should be rechecked against an Abcam PDF/manual
if one becomes available.

## Zymo Research

### ZR small-RNA Ladder — R1090 (RNA)
https://www.govsci.com/product-detail/Zymo--Research/R1090/EA/
29, 25, 21, 17 nt.

## Lonza

### 20 bp DNA Ladder — 50330 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
500, 480, 460, 440, 420, 400, 380, 360, 340, 320, 300, 280, 260, 240, 220, 200, 180, 160, 140, 120, 100, 80, 60, 40, 20 bp.

### 50 bp-1000 bp DNA Marker — 50461 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
1000, 700, 525, 500, 400, 300, 200, 100, 50 bp.

### 100 bp Extended Range DNA Ladder — 50322 (DNA)
https://docslib.org/doc/1955291/section-iv-detection-and-sizing-of-dna-in-agarose-gels
3000 down to 100 bp in 100 bp steps.

### 1 kb-10 kb DNA Marker — 50471 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
10000, 7000, 5000, 4000, 3000, 2500, 2000, 1500, 1000 bp.

---

## Takara Bio

### 20 bp DNA Ladder — 3409A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
**500**, 400, 300, **200**, 180, 160, 140, 120, **100**, 80, 60, 40, 20 bp.

### 100 bp DNA Ladder — 3407A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 200 bp DNA Ladder — 3410A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
4000, 3000, 2500, **2000**, 1800, 1600, 1400, 1200, **1000**, 800, 600, 400, 200 bp.

---

## GoldBio

### ReadyLadder 50 bp–25 kb DNA Ladder — D015 (DNA)
https://www.goldbio.com/product/156/ready-to-use-50-bp-to-25-kb-dna-ladder
25000, 10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1200**, 1000, 900, 800, 700, 600, **500**, 450, 400, 350, 300, 250, **200**, 150, 100, 50 bp.

---

## GenScript

### Broad Multi Color Pre-Stained Protein Standard — M00624S (protein)
https://www.genscript.com/molecule/M00624S-Broad_Multi_Color_Pre_Stained_Protein_Standard.html
**270**, 175, 130, 95, 65, **50**, 35, **30**, 15, 5 kDa.

---

## Abcam

### Prestained Protein Ladder, Broad Molecular Weight — ab116028 (protein)
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-broad-molecular-weight-10-245-kda-ab116028
245, 180, 140, 100, **75**, 60, 45, 35, **25**, 20, 15, 10 kDa.

Note: Abcam's product page publishes the 10–245 kDa range, 12-band count, and
75/25 kDa reference bands; exact apparent sizes are harmonized to the common
vendor-distributed band table for this product family.

---

## Zymo Research

### ZR small-RNA Ladder — R1090 (RNA)
https://www.zymoresearch.com/products/zr-small-rna-ladder
29, 25, 21, 17 nt.

---

## Lonza

### 20 bp DNA Ladder — 50330 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
500, 480, 460, 440, 420, 400, 380, 360, 340, 320, 300, 280, 260, 240, 220, 200, 180, 160, 140, 120, 100, 80, 60, 40, 20 bp.

### 50 bp–1000 bp DNA Marker — 50461 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
1000, 700, 525, 500, 400, 300, 200, 100, 50 bp.

### 100 bp Extended Range DNA Ladder — 50322 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
3000 down to 100 bp in 100 bp increments.

### 1 kb–10 kb DNA Marker — 50471 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
10000, 7000, 5000, 4000, 3000, 2500, 2000, 1500, 1000 bp.
