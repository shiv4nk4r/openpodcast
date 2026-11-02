(() => {
  const top = document.querySelector(".top");
  const mac = document.getElementById("dl-mac");
  const note = document.getElementById("dl-note");

  const onScroll = () => {
    if (!top) return;
    top.classList.toggle("scrolled", window.scrollY > 8);
  };
  onScroll();
  window.addEventListener("scroll", onScroll, { passive: true });

  // Soft-fail if the DMG isn't uploaded yet (local static host / Pages).
  if (mac && note) {
    const href = mac.getAttribute("href");
    fetch(href, { method: "HEAD" })
      .then((res) => {
        if (res.ok) return;
        mac.classList.add("disabled");
        mac.removeAttribute("href");
        mac.setAttribute("aria-disabled", "true");
        mac.querySelector("strong").textContent = "Build not uploaded yet";
        mac.querySelector(".meta").textContent =
          "Add OpenPodcast_0.1.0_aarch64.dmg to website/downloads/";
        note.classList.add("missing");
        note.innerHTML =
          "Place your notarized DMG at <code>website/downloads/OpenPodcast_0.1.0_aarch64.dmg</code>, then redeploy this folder. Until then the Mac button stays disabled.";
      })
      .catch(() => {
        /* file:// or offline — leave the link as-is for local testing */
      });
  }
})();
