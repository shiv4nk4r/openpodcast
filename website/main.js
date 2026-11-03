(() => {
  const top = document.querySelector(".top");

  const onScroll = () => {
    if (!top) return;
    top.classList.toggle("scrolled", window.scrollY > 8);
  };
  onScroll();
  window.addEventListener("scroll", onScroll, { passive: true });
})();
