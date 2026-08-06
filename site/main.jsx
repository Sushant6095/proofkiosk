import React, { Component, useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Canvas, useThree } from '@react-three/fiber';
import {
  Edges,
  Html,
  Line,
  OrbitControls,
  RoundedBox,
} from '@react-three/drei';
import * as THREE from 'three';
import './three-scenes.css';

const COLORS = {
  night: '#0b0c0d',
  panel: '#171b1e',
  steel: '#657078',
  paper: '#f2efe8',
  amber: '#ff9f1c',
  mint: '#48e0b1',
  blue: '#8cc8ff',
  red: '#e45a4f',
  violet: '#a993ff',
};

function useReducedMotion() {
  const [reduced, setReduced] = useState(false);

  useEffect(() => {
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);

  return reduced;
}

function ResponsiveCamera({ desktop = [0, 1.15, 11], compact = [0, 1.15, 16], target = [0, 0.75, 0] }) {
  const { camera, size } = useThree();

  useEffect(() => {
    const next = size.width / size.height < 1.05 ? compact : desktop;
    camera.position.set(...next);
    camera.lookAt(...target);
    camera.updateProjectionMatrix();
  }, [camera, compact, desktop, size.height, size.width, target]);

  return null;
}

class SceneErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { failed: false };
  }

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="scene-fallback" role="img" aria-label={this.props.label}>
          <strong>3D view unavailable</strong>
          <span>The engineering labels and implementation status remain available below.</span>
        </div>
      );
    }
    return this.props.children;
  }
}

function SceneLabel({ position, title, detail, tone = 'neutral', align = 'center' }) {
  return (
    <Html position={position} center={align === 'center'} distanceFactor={7.8} zIndexRange={[40, 0]}>
      <div className={`scene-label ${tone}`}>
        <b>{title}</b>
        {detail && <span>{detail}</span>}
      </div>
    </Html>
  );
}

function Wire({ points, color, dashed = false, width = 2, opacity = 1 }) {
  return (
    <Line
      points={points}
      color={color}
      lineWidth={width}
      dashed={dashed}
      dashSize={0.16}
      gapSize={0.1}
      transparent={opacity < 1}
      opacity={opacity}
    />
  );
}

function Board({ position, size, color, children, radius = 0.08 }) {
  return (
    <group position={position}>
      <RoundedBox args={size} radius={radius} smoothness={4} castShadow receiveShadow>
        <meshStandardMaterial color={color} roughness={0.54} metalness={0.1} />
        <Edges color="#aeb9be" threshold={30} opacity={0.42} transparent />
      </RoundedBox>
      {children}
    </group>
  );
}

function RaspberryPi() {
  const gpioPins = useMemo(
    () => Array.from({ length: 10 }, (_, index) => [-1.05 + index * 0.22, 0.17, -0.66]),
    [],
  );

  return (
    <group position={[-1.75, 0.2, 0.25]}>
      <Board position={[0, 0, 0]} size={[2.7, 0.16, 1.85]} color="#285b45">
        <mesh position={[-0.15, 0.16, 0.05]} castShadow>
          <boxGeometry args={[0.72, 0.18, 0.72]} />
          <meshStandardMaterial color="#111518" metalness={0.42} roughness={0.35} />
        </mesh>
        <mesh position={[0.7, 0.17, 0.18]} castShadow>
          <boxGeometry args={[0.55, 0.2, 0.52]} />
          <meshStandardMaterial color="#1a1f22" metalness={0.28} roughness={0.44} />
        </mesh>
        {gpioPins.map((pin, index) => (
          <mesh key={index} position={pin} castShadow>
            <boxGeometry args={[0.055, 0.26, 0.055]} />
            <meshStandardMaterial color="#d6b44c" metalness={0.8} roughness={0.25} />
          </mesh>
        ))}
        <mesh position={[1.23, 0.18, 0.5]}>
          <boxGeometry args={[0.26, 0.24, 0.64]} />
          <meshStandardMaterial color="#c7cdd0" metalness={0.72} roughness={0.2} />
        </mesh>
        <mesh position={[1.1, 0.2, -0.36]}>
          <boxGeometry args={[0.48, 0.28, 0.34]} />
          <meshStandardMaterial color="#c7cdd0" metalness={0.72} roughness={0.2} />
        </mesh>
      </Board>
      <SceneLabel position={[0, 0.82, 0]} title="Raspberry Pi 4" detail="reference host · no GPIO adapter shipped" tone="mint" />
    </group>
  );
}

function RelayModule() {
  return (
    <group position={[0.65, 0.26, 0.35]}>
      <Board position={[0, 0, 0]} size={[1.6, 0.18, 1.15]} color="#245a86">
        <mesh position={[-0.16, 0.25, 0]} castShadow>
          <boxGeometry args={[0.67, 0.42, 0.72]} />
          <meshStandardMaterial color="#2071aa" roughness={0.44} />
        </mesh>
        {[-0.53, 0, 0.53].map((x, index) => (
          <mesh key={x} position={[x, 0.18, 0.48]} castShadow>
            <boxGeometry args={[0.3, 0.34, 0.28]} />
            <meshStandardMaterial color={index === 1 ? '#474e52' : '#78b6d8'} roughness={0.5} />
          </mesh>
        ))}
      </Board>
      <SceneLabel position={[0, 0.82, 0]} title="1-channel isolated relay" detail="IN ← GPIO17 · dry contacts COM / NO" tone="blue" />
    </group>
  );
}

function BME280() {
  return (
    <group position={[-1.15, 1.7, 0.28]}>
      <Board position={[0, 0, 0]} size={[1.08, 0.12, 0.72]} color="#6b3e98">
        <mesh position={[0.14, 0.16, 0]}>
          <boxGeometry args={[0.3, 0.18, 0.3]} />
          <meshStandardMaterial color="#c9d0d1" metalness={0.7} roughness={0.25} />
        </mesh>
      </Board>
      <SceneLabel position={[0, 0.63, 0]} title="BME280" detail="3.3 V · SDA GPIO2 · SCL GPIO3" tone="violet" />
    </group>
  );
}

function PowerSupply() {
  return (
    <group position={[2.45, 0.25, 0.3]}>
      <Board position={[0, 0, 0]} size={[1.45, 0.72, 1.25]} color="#3c4246">
        <mesh position={[0, 0.38, 0]} rotation={[-Math.PI / 2, 0, 0]}>
          <planeGeometry args={[1.12, 0.9]} />
          <meshStandardMaterial color="#596268" metalness={0.7} roughness={0.32} wireframe />
        </mesh>
        {[-0.28, 0.28].map((x) => (
          <mesh key={x} position={[x, 0.42, -0.48]}>
            <cylinderGeometry args={[0.08, 0.08, 0.12, 18]} />
            <meshStandardMaterial color={x < 0 ? COLORS.red : '#1d2022'} metalness={0.5} />
          </mesh>
        ))}
      </Board>
      <SceneLabel position={[0, 0.95, 0]} title="Separate 12 V supply" detail="never connected to a Pi pin" tone="red" />
    </group>
  );
}

function SolenoidMechanism() {
  return (
    <group position={[1.15, 1.85, 0.25]}>
      <mesh rotation={[0, 0, Math.PI / 2]} castShadow>
        <cylinderGeometry args={[0.36, 0.36, 1.2, 32]} />
        <meshStandardMaterial color="#30383d" metalness={0.72} roughness={0.28} />
      </mesh>
      <mesh position={[0.82, 0, 0]} rotation={[0, 0, Math.PI / 2]} castShadow>
        <cylinderGeometry args={[0.1, 0.1, 0.82, 18]} />
        <meshStandardMaterial color="#d8dfe1" metalness={0.84} roughness={0.18} />
      </mesh>
      <mesh position={[-0.7, 0.48, 0]} rotation={[0, 0, Math.PI / 2]}>
        <cylinderGeometry args={[0.08, 0.08, 0.38, 14]} />
        <meshStandardMaterial color="#16191b" />
      </mesh>
      <mesh position={[-0.7, 0.48, 0.1]} rotation={[Math.PI / 2, 0, 0]}>
        <torusGeometry args={[0.2, 0.028, 10, 28]} />
        <meshStandardMaterial color="#d3d8da" metalness={0.75} />
      </mesh>
      <SceneLabel position={[0.1, 0.82, 0]} title="12 V solenoid / vend motor" detail="1N4007 flyback diode across load" tone="amber" />
    </group>
  );
}

function Chute() {
  return (
    <group position={[3.15, 1.75, 0.1]}>
      <mesh rotation={[0, 0, -0.28]} receiveShadow>
        <boxGeometry args={[1.6, 0.12, 1.05]} />
        <meshStandardMaterial color="#616b70" metalness={0.72} roughness={0.35} />
      </mesh>
      <mesh position={[0.22, 0.42, 0]} rotation={[0, 0, -0.28]} castShadow>
        <boxGeometry args={[0.48, 0.72, 0.54]} />
        <meshStandardMaterial color={COLORS.amber} roughness={0.55} />
      </mesh>
      <SceneLabel position={[-0.42, 1.08, 0]} title="Mechanical vend path" detail="motion is not proof of delivery" />
    </group>
  );
}

function KioskFrame() {
  const posts = [
    [-3.4, 1.25, -1], [3.8, 1.25, -1], [-3.4, 1.25, 1], [3.8, 1.25, 1],
  ];
  return (
    <group>
      {posts.map((position, index) => (
        <mesh key={index} position={position}>
          <boxGeometry args={[0.06, 3.8, 0.06]} />
          <meshStandardMaterial color="#8c969b" transparent opacity={0.28} metalness={0.8} />
        </mesh>
      ))}
      {[[-3.4, 3.15], [3.8, 3.15], [-3.4, -0.65], [3.8, -0.65]].map(([x, y], index) => (
        <mesh key={index} position={[0.2, y, index % 2 ? 1 : -1]}>
          <boxGeometry args={[7.25, 0.06, 0.06]} />
          <meshStandardMaterial color="#8c969b" transparent opacity={0.24} metalness={0.8} />
        </mesh>
      ))}
    </group>
  );
}

function HardwareModel() {
  return (
    <group rotation={[0.08, -0.48, 0]} position={[0, -0.2, 0]}>
      <KioskFrame />
      <RaspberryPi />
      <RelayModule />
      <BME280 />
      <PowerSupply />
      <SolenoidMechanism />
      <Chute />

      <Wire points={[[-0.55, 0.2, -0.15], [-0.2, 0.2, -0.15], [-0.2, 0.36, -0.15]]} color={COLORS.amber} width={2.5} />
      <Wire points={[[-0.7, 0.14, -0.04], [-0.15, 0.14, -0.04], [-0.15, 0.28, -0.04]]} color={COLORS.red} />
      <Wire points={[[-0.8, 0.08, 0.08], [-0.08, 0.08, 0.08], [-0.08, 0.22, 0.08]]} color="#899297" />

      <Wire points={[[-1.9, 0.36, 0.8], [-1.9, 1.26, 0.8], [-1.15, 1.34, 0.64]]} color={COLORS.violet} width={2.2} />
      <Wire points={[[-1.68, 0.36, 0.72], [-1.68, 1.18, 0.72], [-1.0, 1.35, 0.52]]} color={COLORS.blue} />
      <Wire points={[[-1.46, 0.36, 0.64], [-1.46, 1.1, 0.64], [-0.86, 1.35, 0.4]]} color={COLORS.mint} />

      <Wire points={[[1.35, 0.42, 0.55], [1.35, 1.02, 0.55], [0.6, 1.46, 0.52]]} color={COLORS.red} width={2.5} />
      <Wire points={[[2.1, 0.5, 0.5], [1.75, 0.5, 0.5], [1.75, 1.34, 0.5], [1.65, 1.58, 0.45]]} color={COLORS.red} width={2.5} />
      <Wire points={[[2.72, 0.48, 0.2], [2.72, 1.12, 0.2], [1.68, 1.62, 0.2]]} color="#30383d" width={2.5} />

      <SceneLabel position={[-0.2, -0.15, 0.85]} title="GPIO side" detail="5 V pin 2 · GPIO17 pin 11 · GND pin 6" tone="amber" />
      <SceneLabel position={[1.72, -0.22, 0.92]} title="Load side" detail="dry contacts isolated from Pi logic" tone="red" />
    </group>
  );
}

function HardwareScene() {
  const reducedMotion = useReducedMotion();
  return (
    <div className="three-stage hardware-stage">
      <Canvas
        shadows
        dpr={[1, 1.6]}
        camera={{ position: [0, 1.15, 11], fov: 38, near: 0.1, far: 100 }}
        gl={{ antialias: true, alpha: true, powerPreference: 'high-performance' }}
        fallback={<div className="scene-fallback">WebGL unavailable</div>}
      >
        <color attach="background" args={[COLORS.night]} />
        <ambientLight intensity={1.25} />
        <hemisphereLight args={['#d6e8ff', '#24180b', 1.4]} />
        <directionalLight position={[4, 8, 7]} intensity={2.5} castShadow shadow-mapSize={[1024, 1024]} />
        <pointLight position={[-5, 2, 5]} intensity={15} color={COLORS.mint} distance={10} />
        <pointLight position={[4, 3, 4]} intensity={18} color={COLORS.amber} distance={9} />
        <ResponsiveCamera />
        <HardwareModel />
        <gridHelper args={[12, 24, '#293034', '#171b1e']} position={[0, -1.02, 0]} />
        <OrbitControls
          makeDefault
          enablePan={false}
          minDistance={7.5}
          maxDistance={14}
          target={[0.1, 0.75, 0]}
          minPolarAngle={0.55}
          maxPolarAngle={1.55}
          autoRotate={!reducedMotion}
          autoRotateSpeed={0.34}
        />
      </Canvas>
      <div className="scene-corner scene-corner-top">
        <b>Reference wiring · interactive</b>
        <span>Drag to rotate · scroll to zoom</span>
      </div>
      <div className="scene-key" aria-label="Hardware implementation status">
        <span><i className="implemented" /> documented pin map</span>
        <span><i className="roadmap" /> adapters not shipped</span>
      </div>
    </div>
  );
}

const PLUGIN_COLORS = {
  charge: COLORS.amber,
  watch: COLORS.mint,
  attest: COLORS.violet,
};

function DiagramNode({ position, size = [2.05, 0.76, 0.44], color = COLORS.steel, title, detail, tone, active, onClick, roadmap }) {
  return (
    <group position={position} onClick={onClick}>
      <RoundedBox args={size} radius={0.12} smoothness={4} castShadow>
        <meshStandardMaterial
          color={color}
          emissive={active ? color : COLORS.night}
          emissiveIntensity={active ? 0.32 : 0.03}
          metalness={0.28}
          roughness={0.42}
          transparent={roadmap}
          opacity={roadmap ? 0.66 : 1}
        />
        <Edges color={active ? '#ffffff' : '#8c969b'} threshold={24} opacity={active ? 0.8 : 0.28} transparent />
      </RoundedBox>
      <Html center distanceFactor={8.4} zIndexRange={[45, 0]}>
        <button
          className={`diagram-node ${tone || ''} ${active ? 'active' : ''} ${roadmap ? 'roadmap' : ''}`}
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onClick?.();
          }}
        >
          <b>{title}</b>
          <span>{detail}</span>
        </button>
      </Html>
    </group>
  );
}

function ArchitectureModel({ active, select }) {
  return (
    <group position={[0, 0.25, 0]}>
      <DiagramNode position={[-4.65, 2.25, 0]} size={[1.8, 0.7, 0.42]} title="Channel" detail="customer request" color="#465159" />
      <DiagramNode position={[-2.25, 0.5, 0]} size={[2.2, 2.35, 0.58]} title="ZeroClaw" detail="pinned plugin host" color="#2f383d" />
      <DiagramNode position={[0.45, 2.05, 0]} title="kiosk-charge" detail="T1 · config_read · no HTTP" color={PLUGIN_COLORS.charge} tone="amber" active={active === 'charge'} onClick={() => select('charge')} />
      <DiagramNode position={[0.45, 0.45, 0]} title="kiosk-watch" detail="T0 · finalized RPC read" color={PLUGIN_COLORS.watch} tone="mint" active={active === 'watch'} onClick={() => select('watch')} />
      <DiagramNode position={[0.45, -1.15, 0]} title="kiosk-attest" detail="T1 · unsigned Memo tx" color={PLUGIN_COLORS.attest} tone="violet" active={active === 'attest'} onClick={() => select('attest')} />
      <DiagramNode position={[3.55, 2.25, 0]} size={[1.9, 0.72, 0.44]} title="Customer wallet" detail="external signer" color="#547498" />
      <DiagramNode position={[4.45, 0.35, 0]} size={[2.0, 1.18, 0.62]} title="Solana" detail="finalized ledger" color="#4864a8" />
      <DiagramNode position={[3.8, -1.45, 0]} size={[1.85, 0.7, 0.44]} title="Operator RPC" detail="configured trust root" color="#475157" />
      <DiagramNode position={[2.85, -2.75, 0]} size={[2.0, 0.72, 0.44]} title="External signer" detail="not shipped" color="#655677" roadmap />
      <DiagramNode position={[-2.2, -1.85, 0]} size={[2.2, 0.74, 0.44]} title="Persist + claim" detail="raw host-direct result" color="#4f5b60" />
      <DiagramNode position={[-4.65, -2.55, 0]} size={[2.0, 0.78, 0.44]} title="Driver → hardware" detail="integration contract" color="#7b5427" roadmap />

      <Wire points={[[-3.75, 2.05, 0], [-3.1, 1.55, 0], [-2.6, 1.25, 0]]} color="#859198" />
      <Wire points={[[-1.15, 1.25, 0], [-0.65, 1.7, 0], [-0.58, 1.96, 0]]} color={PLUGIN_COLORS.charge} width={2.4} />
      <Wire points={[[-1.15, 0.5, 0], [-0.6, 0.5, 0]]} color={PLUGIN_COLORS.watch} width={2.4} />
      <Wire points={[[-1.15, -0.3, 0], [-0.65, -0.7, 0], [-0.58, -1.02, 0]]} color={PLUGIN_COLORS.attest} width={2.4} />

      <Wire points={[[1.5, 2.05, 0], [2.35, 2.25, 0], [2.62, 2.25, 0]]} color={PLUGIN_COLORS.charge} width={2.5} />
      <Wire points={[[4.05, 1.88, 0], [4.35, 1.35, 0], [4.38, 0.95, 0]]} color={COLORS.blue} width={2.5} />

      <Wire points={[[1.52, 0.45, 0], [2.35, 0.45, 0], [3.42, 0.45, 0]]} color={PLUGIN_COLORS.watch} width={2.5} />
      <Wire points={[[3.8, -1.1, 0], [4.1, -0.75, 0], [4.22, -0.28, 0]]} color="#869197" width={2} />

      <Wire points={[[1.5, -1.15, 0], [2.2, -1.5, 0], [2.55, -2.35, 0]]} color={PLUGIN_COLORS.attest} width={2.5} />
      <Wire points={[[3.45, -2.45, 0], [4.15, -1.9, 0], [4.35, -0.28, 0]]} color={PLUGIN_COLORS.attest} dashed opacity={0.72} />

      <Wire points={[[-1.72, -0.68, 0], [-1.85, -1.15, 0], [-2.05, -1.48, 0]]} color={COLORS.amber} />
      <Wire points={[[-3.3, -2.0, 0], [-3.85, -2.28, 0]]} color={COLORS.amber} dashed opacity={0.7} />

      <SceneLabel position={[3.18, 1.33, 0]} title="customer signs" tone="blue" />
      <SceneLabel position={[2.4, 0.83, 0]} title="read-only evidence" tone="mint" />
      <SceneLabel position={[3.4, -2.08, 0]} title="unsigned → external" tone="violet" />
      <SceneLabel position={[-3.72, -1.55, 0]} title="dashed = not shipped" tone="amber" />
    </group>
  );
}

function ArchitectureScene() {
  const [active, setActive] = useState('charge');
  const reducedMotion = useReducedMotion();

  useEffect(() => {
    const update = (event) => setActive(event.detail);
    window.addEventListener('proofkiosk:plugin-selected', update);
    return () => window.removeEventListener('proofkiosk:plugin-selected', update);
  }, []);

  const select = (key) => {
    setActive(key);
    if (typeof window.selectPlugin === 'function') window.selectPlugin(key);
  };

  return (
    <div className="three-stage architecture-stage">
      <Canvas
        dpr={[1, 1.5]}
        camera={{ position: [0.2, 0.4, 13.5], fov: 48, near: 0.1, far: 80 }}
        gl={{ antialias: true, alpha: true, powerPreference: 'high-performance' }}
        fallback={<div className="scene-fallback">WebGL unavailable</div>}
      >
        <color attach="background" args={['#111416']} />
        <ambientLight intensity={1.5} />
        <directionalLight position={[4, 7, 8]} intensity={2.5} />
        <pointLight position={[-4, 2, 5]} intensity={12} color={COLORS.mint} distance={11} />
        <pointLight position={[4, 2, 5]} intensity={14} color={COLORS.amber} distance={11} />
        <ResponsiveCamera desktop={[0.2, 0.4, 13.5]} compact={[0.2, 0.4, 17]} target={[0, 0, 0]} />
        <ArchitectureModel active={active} select={select} />
        <OrbitControls
          makeDefault
          enablePan={false}
          enableZoom
          minDistance={11}
          maxDistance={18}
          minAzimuthAngle={-0.34}
          maxAzimuthAngle={0.34}
          minPolarAngle={1.18}
          maxPolarAngle={1.88}
          autoRotate={false}
          enableDamping={!reducedMotion}
        />
      </Canvas>
      <div className="scene-corner scene-corner-top">
        <b>Runtime topology · interactive</b>
        <span>Select a plugin node to inspect its boundary</span>
      </div>
      <div className="scene-key" aria-label="Architecture diagram legend">
        <span><i className="solid" /> implemented data path</span>
        <span><i className="roadmap" /> external / roadmap path</span>
      </div>
    </div>
  );
}

function mountScene(id, element, label) {
  const node = document.getElementById(id);
  if (!node) return;
  createRoot(node).render(<SceneErrorBoundary label={label}>{element}</SceneErrorBoundary>);
}

mountScene(
  'hardware-scene',
  <HardwareScene />,
  'Interactive 3D reference wiring for a Raspberry Pi 4, isolated relay, BME280, separate 12 volt supply, and solenoid load',
);
mountScene(
  'architecture-scene',
  <ArchitectureScene />,
  'Interactive 3D topology of ZeroClaw, kiosk-charge, kiosk-watch, kiosk-attest, external wallet, Solana, and roadmap hardware boundary',
);
