"use client";
import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { createDitheredComposer } from "../shared-dither";

/**
 * Hero right-panel 3D scene reproducing the exact initial ('idle') state of
 * `frontend/src/components/AngelScene.ts`:
 *  - Split rectangular stone monolith (BoxGeometry 3.5 × 18 × 4, two halves)
 *  - Fluid water ripple plane underneath (y = -9)
 *  - Vertical reticle antennas, crosshairs, rings, branching circuit lines,
 *    scattered floating '+' crosses, and 2,500 stardust particles
 *  - Optional click-triggered eruption revealing the 14-blade cyber wings & halos
 */
export function AngelHeroCanvas({ erupted }: { erupted: boolean }) {
  const mountRef = useRef<HTMLDivElement>(null);
  const eruptedRef = useRef(erupted);
  eruptedRef.current = erupted;
  const [webglOk, setWebglOk] = useState(true);

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;
    const reducedMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
    } catch {
      setWebglOk(false);
      return;
    }

    const getBounds = () => ({
      w: Math.max(mount.clientWidth, 320),
      h: Math.max(mount.clientHeight, 440),
    });
    const { w: W, h: H } = getBounds();

    renderer.setSize(W, H);
    renderer.setPixelRatio(window.devicePixelRatio || 1);
    renderer.setClearColor(0xffffff, 1);
    mount.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0xf0f0f0);

    // Centered inside the right hero panel, matching AngelScene's vertical framing
    const camera = new THREE.PerspectiveCamera(50, W / H, 0.1, 150);
    const baseCamPos = new THREE.Vector3(0, -2.5, 34);
    camera.position.copy(baseCamPos);
    const baseLookTarget = new THREE.Vector3(0, 0, 0);
    camera.lookAt(baseLookTarget);

    // Exact lighting rig from AngelScene.ts
    scene.add(new THREE.AmbientLight(0xffffff, 0.2));

    const keyLight = new THREE.DirectionalLight(0xffffff, 4.0);
    keyLight.position.set(-15, 20, 15);
    scene.add(keyLight);

    const rimLight = new THREE.DirectionalLight(0xffffff, 2.0);
    rimLight.position.set(15, -5, -15);
    scene.add(rimLight);

    const fillLight = new THREE.DirectionalLight(0xffffff, 0.4);
    fillLight.position.set(0, 5, 5);
    scene.add(fillLight);

    // 1. Stone procedural texture (exact match to AngelScene.ts)
    function createStoneTexture() {
      const canvas = document.createElement("canvas");
      canvas.width = 512;
      canvas.height = 512;
      const ctx = canvas.getContext("2d")!;
      ctx.fillStyle = "#888888";
      ctx.fillRect(0, 0, canvas.width, canvas.height);

      for (let i = 0; i < 400; i++) {
        ctx.fillStyle =
          Math.random() > 0.5
            ? "rgba(255,255,255,0.04)"
            : "rgba(0,0,0,0.04)";
        ctx.beginPath();
        const x = Math.random() * canvas.width;
        const y = Math.random() * canvas.height;
        const r = Math.random() * 80 + 20;
        ctx.arc(x, y, r, 0, Math.PI * 2);
        ctx.fill();
      }

      for (let i = 0; i < 150000; i++) {
        const x = Math.random() * canvas.width;
        const y = Math.random() * canvas.height;
        ctx.fillStyle =
          Math.random() > 0.5
            ? "rgba(255,255,255,0.06)"
            : "rgba(0,0,0,0.06)";
        ctx.fillRect(x, y, 1, 1);
      }

      const tex = new THREE.CanvasTexture(canvas);
      tex.anisotropy = renderer.capabilities.getMaxAnisotropy();
      tex.minFilter = THREE.LinearMipmapLinearFilter;
      tex.magFilter = THREE.LinearFilter;
      tex.wrapS = THREE.RepeatWrapping;
      tex.wrapT = THREE.RepeatWrapping;
      return tex;
    }

    const monoTex = createStoneTexture();
    const solidMat = new THREE.MeshStandardMaterial({
      color: 0xcccccc,
      map: monoTex,
      bumpMap: monoTex,
      bumpScale: 0.05,
      roughness: 0.95,
      metalness: 0.0,
      emissive: 0x222222,
    });

    // 2. Faceted geometric crystal blades & crisp architectural astrolabe rings
    const bladeMatPrimary = new THREE.MeshStandardMaterial({
      color: 0x949494,
      map: monoTex,
      roughness: 0.78,
      metalness: 0.1,
      flatShading: true,
      emissive: 0x161616,
    });
    const bladeMatSecondary = new THREE.MeshStandardMaterial({
      color: 0xb6b6b6,
      roughness: 0.85,
      metalness: 0.05,
      flatShading: true,
      emissive: 0x1e1e1e,
    });

    const ringSolidMat = new THREE.MeshBasicMaterial({ color: 0x1a1a1a });
    const ringSoftMat = new THREE.MeshBasicMaterial({ color: 0x666666 });

    const sceneRoot = new THREE.Group();
    scene.add(sceneRoot);

    // Sacred Astrolabe Halo strictly BEHIND the monolith (z = -7, flat XY plane + subtle tilt)
    const halos = new THREE.Group();
    halos.position.set(0, 1.5, -7);
    halos.scale.set(0, 0, 0);
    halos.visible = false;
    sceneRoot.add(halos);

    const haloOuter = new THREE.Group();
    haloOuter.add(
      new THREE.Mesh(new THREE.TorusGeometry(9.6, 0.055, 8, 96), ringSolidMat),
    );
    // 12 precision clockwork tick marks around the outer halo
    for (let i = 0; i < 12; i++) {
      const a = (i / 12) * Math.PI * 2;
      const len = i % 3 === 0 ? 0.85 : 0.42;
      const tick = new THREE.Mesh(
        new THREE.BoxGeometry(0.055, len, 0.055),
        i % 3 === 0 ? ringSolidMat : ringSoftMat,
      );
      tick.position.set(Math.cos(a) * 9.6, Math.sin(a) * 9.6, 0);
      tick.rotation.z = a - Math.PI / 2;
      haloOuter.add(tick);
    }
    // 4 cardinal diamond nodes
    for (let i = 0; i < 4; i++) {
      const a = (i / 4) * Math.PI * 2 + Math.PI / 4;
      const node = new THREE.Mesh(
        new THREE.RingGeometry(0.22, 0.3, 4),
        ringSolidMat,
      );
      node.position.set(Math.cos(a) * 9.6, Math.sin(a) * 9.6, 0);
      haloOuter.add(node);
    }
    halos.add(haloOuter);

    const haloInner = new THREE.Group();
    haloInner.add(
      new THREE.Mesh(new THREE.TorusGeometry(6.8, 0.045, 8, 80), ringSoftMat),
    );
    // Inscribed octagonal sacred frame
    const octRing = new THREE.Mesh(
      new THREE.RingGeometry(6.15, 6.22, 8),
      ringSoftMat,
    );
    haloInner.add(octRing);
    halos.add(haloInner);

    // Exact initial Monolith shape from AngelScene.ts
    const monolithGroup = new THREE.Group();
    sceneRoot.add(monolithGroup);

    const halfGeo = new THREE.BoxGeometry(3.5, 18, 4, 3, 12, 2);
    halfGeo.translate(1.75, 0, 0);
    halfGeo.computeVertexNormals();

    const monoLeft = new THREE.Mesh(halfGeo, solidMat);
    monoLeft.scale.x = -1;
    const monoRight = new THREE.Mesh(halfGeo, solidMat);
    monolithGroup.add(monoLeft, monoRight);

    // Crisp low-poly diamond crystal shard (detail=0 => sharp facets for Bayer dither!)
    const bladeGeo = new THREE.OctahedronGeometry(1, 0);
    bladeGeo.scale(0.46, 4.4, 0.2);
    bladeGeo.translate(0, 2.2, 0);
    bladeGeo.computeVertexNormals();

    function createSeraphWing(isLeft: boolean) {
      const group = new THREE.Group();
      const dir = isLeft ? -1 : 1;
      const feathers: THREE.Mesh[] = [];

      // Upper primary pinion fan (10 long sweeping blades)
      const primaryCount = 10;
      for (let i = 0; i < primaryCount; i++) {
        const f = i / (primaryCount - 1);
        const blade = new THREE.Mesh(bladeGeo, bladeMatPrimary);
        // Shoulder anchor curves gently upward & outward, blades radiate from the shoulder
        const px = dir * (2.1 + f * 3.1);
        const py = 1.6 + Math.sin(f * Math.PI * 0.85) * 2.6 - f * 2.2;
        const pz = -2.2 - f * 1.8;

        // Inner blades sweep steeply upward (~24° outward), outer blades fan gracefully (~96° outward)
        const rz = -dir * (0.42 + f * 1.26);
        const ry = dir * (0.14 + f * 0.28);
        const rx = 0.08 + f * 0.14;

        const len = 1.95 - Math.pow(f, 1.35) * 0.88;
        const wid = 0.92 - f * 0.28;

        blade.userData = {
          tPos: new THREE.Vector3(px, py, pz),
          tRot: new THREE.Euler(rx, ry, rz),
          tScale: new THREE.Vector3(wid, len, 1.0),
        };
        blade.scale.set(0.01, 0.01, 0.01);
        blade.position.set(dir * 1.4, 0.5, -2.0);
        feathers.push(blade);
        group.add(blade);
      }

      // Lower secondary covert fan (6 shorter inner blades for layered seraph depth)
      const secondaryCount = 6;
      for (let j = 0; j < secondaryCount; j++) {
        const f = j / (secondaryCount - 1);
        const blade = new THREE.Mesh(bladeGeo, bladeMatSecondary);
        const px = dir * (2.0 + f * 2.2);
        const py = -1.2 - f * 1.6;
        const pz = -1.6 - f * 1.1;

        const rz = -dir * (1.28 + f * 0.62);
        const ry = dir * (0.1 + f * 0.18);
        const rx = 0.06;

        const len = 1.12 - f * 0.42;
        const wid = 0.78 - f * 0.2;

        blade.userData = {
          tPos: new THREE.Vector3(px, py, pz),
          tRot: new THREE.Euler(rx, ry, rz),
          tScale: new THREE.Vector3(wid, len, 0.9),
        };
        blade.scale.set(0.01, 0.01, 0.01);
        blade.position.set(dir * 1.4, -1.0, -1.6);
        feathers.push(blade);
        group.add(blade);
      }

      return { group, feathers };
    }

    const wingL = createSeraphWing(true);
    const wingR = createSeraphWing(false);
    wingL.group.visible = false;
    wingR.group.visible = false;
    sceneRoot.add(wingL.group, wingR.group);

    // 3. Atmosphere & Sacred Geometry (exact match to AngelScene.ts)
    const atmosGroup = new THREE.Group();
    sceneRoot.add(atmosGroup);

    const matSolid = new THREE.MeshBasicMaterial({ color: 0x111111 });
    const matDotted = new THREE.MeshBasicMaterial({ color: 0x999999 });
    const matAccent = new THREE.MeshBasicMaterial({ color: 0x444444 });

    const createLine = (
      x: number,
      y: number,
      z: number,
      w: number,
      h: number,
      d: number,
      mat: THREE.Material = matSolid,
    ) => {
      const mesh = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), mat);
      mesh.position.set(x, y, z);
      return mesh;
    };

    // Fluid Water Ripple Floor
    const waterGeo = new THREE.PlaneGeometry(40, 40, 128, 128);
    waterGeo.rotateX(-Math.PI / 2);
    const waterMat = new THREE.MeshStandardMaterial({
      color: 0x999999,
      roughness: 0.2,
      metalness: 0.8,
      transparent: true,
      opacity: 0.9,
      depthWrite: false,
    });
    const waterUniforms = {
      uTime: { value: 0 },
      uImpact: { value: 0 },
    };
    waterMat.onBeforeCompile = (shader) => {
      shader.uniforms.uTime = waterUniforms.uTime;
      shader.uniforms.uImpact = waterUniforms.uImpact;
      shader.vertexShader = `
        uniform float uTime;
        uniform float uImpact;
        varying float vDist;
        float getWave(float d, float time, float impact) {
          float wave1 = sin(d * 1.5 - time * 2.0) * 0.15;
          float wave2 = sin(d * 2.5 - time * 3.0) * impact * 0.4;
          float mask = smoothstep(1.5, 3.0, d) * smoothstep(18.0, 5.0, d);
          return (wave1 + wave2) * mask;
        }
        ${shader.vertexShader}
      `
        .replace(
          `#include <beginnormal_vertex>`,
          `
          float dist = length(position.xz);
          vDist = dist;
          float h = getWave(dist, uTime, uImpact);
          float delta = 0.05;
          float hDx = getWave(length(position.xz + vec2(delta, 0.0)), uTime, uImpact);
          float hDz = getWave(length(position.xz + vec2(0.0, delta)), uTime, uImpact);
          vec3 tangent = normalize(vec3(delta, hDx - h, 0.0));
          vec3 bitangent = normalize(vec3(0.0, hDz - h, delta));
          vec3 objectNormal = normalize(cross(bitangent, tangent));
          `,
        )
        .replace(
          `#include <begin_vertex>`,
          `vec3 transformed = vec3( position.x, position.y + h, position.z );`,
        );

      shader.fragmentShader = `
        varying float vDist;
        ${shader.fragmentShader}
      `.replace(
        `vec4 diffuseColor = vec4( diffuse, opacity );`,
        `
        float alpha = smoothstep(20.0, 10.0, vDist) * smoothstep(1.5, 3.0, vDist);
        vec4 diffuseColor = vec4( diffuse, opacity * alpha );
        `,
      );
    };
    const waterSurface = new THREE.Mesh(waterGeo, waterMat);
    waterSurface.position.y = -9;
    atmosGroup.add(waterSurface);

    // Vertical Axis & Crosshairs
    atmosGroup.add(createLine(0, 13, 0, 0.06, 8, 0.06, matDotted));
    atmosGroup.add(createLine(0, 16.5, 0, 1.2, 0.06, 0.06, matSolid));
    atmosGroup.add(createLine(0, 16.5, 0, 0.06, 1.2, 0.06, matSolid));
    const topRing = new THREE.Mesh(new THREE.RingGeometry(0.3, 0.38, 32), matSolid);
    topRing.position.set(0, 14.5, 0);
    atmosGroup.add(topRing);

    atmosGroup.add(createLine(0, -12, 0, 0.06, 6, 0.06, matDotted));
    atmosGroup.add(createLine(0, -14, 0, 1.0, 0.06, 0.06, matSolid));
    atmosGroup.add(createLine(0, -14, 0, 0.06, 1.0, 0.06, matSolid));
    const botRing = new THREE.Mesh(new THREE.RingGeometry(0.4, 0.48, 32), matSolid);
    botRing.position.set(0, -11, 0);
    atmosGroup.add(botRing);

    // Tech/Circuit Lines Branching Out
    const addCircuitNode = (isLeft: boolean) => {
      const dir = isLeft ? -1 : 1;
      const group = new THREE.Group();
      group.add(createLine(dir * 3.5, -4, 0, 3.5, 0.05, 0.05, matSolid));
      group.add(createLine(dir * 5.25, -4.5, 0, 0.05, 1.0, 0.05, matSolid));
      group.add(createLine(dir * 7.0, -5, 0, 3.5, 0.05, 0.05, matSolid));

      const sq = new THREE.Mesh(new THREE.RingGeometry(0.3, 0.38, 4), matSolid);
      sq.rotation.z = Math.PI / 4;
      sq.position.set(dir * 8.75, -5, 0);
      group.add(sq);

      group.add(createLine(dir * 9.5, -5, 0, 0.3, 0.05, 0.05, matAccent));
      group.add(createLine(dir * 9.5, -5, 0, 0.05, 0.3, 0.05, matAccent));
      group.add(createLine(dir * 5.0, -7, 0, 4.0, 0.05, 0.05, matDotted));

      const dot = new THREE.Mesh(new THREE.BoxGeometry(0.2, 0.2, 0.2), matSolid);
      dot.position.set(dir * 7.0, -7, 0);
      group.add(dot);
      return group;
    };
    atmosGroup.add(addCircuitNode(true));
    atmosGroup.add(addCircuitNode(false));

    // Scattered Floating Crosses (+)
    const crossGroup = new THREE.Group();
    for (let i = 0; i < 15; i++) {
      const x = (Math.random() - 0.5) * 40;
      const y = (Math.random() - 0.5) * 30 + 5;
      const z = (Math.random() - 0.5) * 10 - 5;
      if (Math.abs(x) < 6 && Math.abs(y) < 10) continue;
      const size = 0.2 + Math.random() * 0.3;
      const cross = new THREE.Group();
      cross.add(createLine(0, 0, 0, size, 0.04, 0.04, matAccent));
      cross.add(createLine(0, 0, 0, 0.04, size, 0.04, matAccent));
      cross.position.set(x, y, z);
      crossGroup.add(cross);
    }
    atmosGroup.add(crossGroup);

    // Stardust / Particle Field
    const particleCount = 2500;
    const pGeo = new THREE.BufferGeometry();
    const pPos = new Float32Array(particleCount * 3);
    for (let i = 0; i < particleCount; i++) {
      const radius = 5 + Math.random() * 25;
      const theta = (Math.random() - 0.5) * Math.PI * 0.8;
      pPos[i * 3] = Math.sin(theta) * radius;
      pPos[i * 3 + 1] =
        -6 + Math.cos(theta) * radius + (Math.random() - 0.5) * 12;
      pPos[i * 3 + 2] = (Math.random() - 0.5) * 8 - 4;
    }
    pGeo.setAttribute("position", new THREE.BufferAttribute(pPos, 3));
    const pMat = new THREE.PointsMaterial({ size: 0.1, color: 0x888888 });
    sceneRoot.add(new THREE.Points(pGeo, pMat));

    const dc = createDitheredComposer(renderer, scene, camera, W, H, true);
    dc.setExposure(1.8);

    let eruptionProgress = 0;
    let monolithAngle = 0;
    let lastTime = performance.now();
    let rafId = 0;
    const baseTime = lastTime;

    const easeInOutCubic = (v: number) =>
      v < 0.5 ? 4 * v * v * v : 1 - Math.pow(-2 * v + 2, 3) / 2;

    function renderFrame(now: number) {
      const dt = Math.min(Math.max((now - lastTime) * 0.001, 0), 0.1);
      lastTime = now;
      const t = reducedMotion ? 0.6 : (now - baseTime) * 0.001;
      waterUniforms.uTime.value = t;

      // Smooth linear progress + cubic ease-in-out instead of violent exponential snap
      const targetErupt = eruptedRef.current ? 1 : 0;
      const step = dt * 0.9;
      if (eruptionProgress < targetErupt) {
        eruptionProgress = Math.min(targetErupt, eruptionProgress + step);
      } else if (eruptionProgress > targetErupt) {
        eruptionProgress = Math.max(targetErupt, eruptionProgress - step);
      }
      const e = easeInOutCubic(eruptionProgress);

      waterUniforms.uImpact.value = e * 0.5;

      // Keep the monolith calm: gently slow its drift to a pause when wings open,
      // never snapping or rewinding its angle.
      if (!reducedMotion) {
        monolithAngle += dt * 0.14 * (1 - e * 0.85);
      }

      monolithGroup.position.y = Math.sin(t) * 0.4;
      monolithGroup.rotation.y = monolithAngle;
      camera.position.y = baseCamPos.y + Math.sin(t * 0.5) * 0.3;
      // Pull camera slightly back from 34 -> 38 when wings open so the full span fits cleanly
      camera.position.z = THREE.MathUtils.lerp(baseCamPos.z, 38, e);

      const showWings = e > 0.005;
      wingL.group.visible = showWings;
      wingR.group.visible = showWings;
      halos.visible = showWings;

      if (showWings) {
        halos.scale.setScalar(e);
        // Flat counter-rotating sacred astrolabe behind the monolith (no 3D tumbling across front!)
        haloOuter.rotation.z = -t * 0.1;
        haloInner.rotation.z = t * 0.16;

        for (let i = 0; i < wingL.feathers.length; i++) {
          const bl = wingL.feathers[i];
          const br = wingR.feathers[i];
          const breath = Math.sin(t * 1.8 + i * 0.28) * 0.045 * e;
          const sway = Math.cos(t * 1.4 + i * 0.22) * 0.025 * e;

          bl.scale.lerpVectors(
            new THREE.Vector3(0.02, 0.02, 0.02),
            bl.userData.tScale,
            e,
          );
          br.scale.copy(bl.scale);

          bl.position.lerpVectors(
            new THREE.Vector3(-1.2, 0.2, -1.8),
            bl.userData.tPos,
            e,
          );
          br.position.copy(bl.position).setX(-bl.position.x);

          bl.rotation.set(
            THREE.MathUtils.lerp(0, bl.userData.tRot.x, e) + breath,
            THREE.MathUtils.lerp(0, bl.userData.tRot.y, e),
            THREE.MathUtils.lerp(-0.25, bl.userData.tRot.z, e) + sway,
          );
          br.rotation.set(bl.rotation.x, -bl.rotation.y, -bl.rotation.z);
        }
      }

      camera.lookAt(baseLookTarget);
      dc.composer.render();

      if (!reducedMotion) {
        rafId = requestAnimationFrame(renderFrame);
      }
    }

    rafId = requestAnimationFrame(renderFrame);

    const resizeObserver = new ResizeObserver(() => {
      const { w: nW, h: nH } = getBounds();
      renderer.setPixelRatio(window.devicePixelRatio || 1);
      renderer.setSize(nW, nH);
      dc.setSize(nW, nH);
      camera.aspect = nW / nH;
      camera.updateProjectionMatrix();
      if (reducedMotion) renderFrame(performance.now());
    });
    resizeObserver.observe(mount);

    return () => {
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
      if (mount.contains(renderer.domElement)) {
        mount.removeChild(renderer.domElement);
      }
      monoTex.dispose();
      halfGeo.dispose();
      bladeGeo.dispose();
      waterGeo.dispose();
      pGeo.dispose();
      renderer.forceContextLoss();
      renderer.dispose();
    };
  }, []);

  if (!webglOk) {
    return (
      <div className="enox-hero-fallback" aria-hidden="true">
        <img src="/img/logo.svg" alt="" width={140} height={140} />
      </div>
    );
  }

  return <div ref={mountRef} className="enox-angel-canvas" aria-hidden="true" />;
}
