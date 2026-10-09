"""Measure production cache PCM, never a Python reimplementation of the DSP."""
import json, math
from pathlib import Path
import numpy as np
root=Path('tests/generated/stretch')
records=json.loads((root/'renders.json').read_text())
results=[]
def pcm(r):return np.fromfile(root/r['file'],dtype='<f4').reshape(-1,2).astype(float)
def rms(x):return float(np.sqrt(np.mean(x*x)))
def db(x):return 20*math.log10(max(1e-15,x))
def bands(x,rate):
    n=8192;win=np.hanning(n);total=np.zeros(n//2+1);count=0
    for at in range(0,len(x)-n+1,n//2):
        total+=np.sum(abs(np.fft.rfft(x[at:at+n]*win[:,None],axis=0))**2,axis=1);count+=1
    freq=np.fft.rfftfreq(n,1/rate)
    return np.array([total[(freq>=lo)&(freq<hi)].sum()/max(1,count) for lo,hi in zip([20,80,250,1000,4000],[80,250,1000,4000,20000])])
for r in records:
    x=pcm(r);rate=r['rate'];assert np.isfinite(x).all();assert len(x)==r['stats']['frames']
    v={**r,'peakDbfs':db(float(abs(x).max())),'rmsDbfs':db(rms(x)),'realtimeFactor':r['stats']['elapsedMs']/1000/(len(x)/rate)}
    if r['kind']=='tone':
        a=int(len(x)*.2);b=int(len(x)*.8);y=x[a:b,0];n=len(y);win=np.hanning(n)
        fft_n=1<<(max(131072,n*4)-1).bit_length();spec=abs(np.fft.rfft(y*win,n=fft_n));k=int(np.argmax(spec))
        z=np.log(spec[k-1:k+2]+1e-30);offset=.5*(z[0]-z[2])/(z[0]-2*z[1]+z[2]);hz=(k+offset)*rate/fft_n
        t=np.arange(n)/rate;basis=np.column_stack([np.sin(2*np.pi*hz*t),np.cos(2*np.pi*hz*t),np.ones(n)])
        coeff=np.linalg.lstsq(basis,y,rcond=None)[0];residual=y-basis@coeff
        v.update(measuredHz=hz,pitchErrorCents=1200*math.log2(hz/r['frequency']),levelDeltaDb=db(rms(y)/(.2/math.sqrt(2))),residualDb=db(rms(residual)/rms(y)),stereoNullPeak=float(abs(x[:,0]+x[:,1]).max()),steadyStereoNullPeak=float(abs(x[a:b,0]+x[a:b,1]).max()))
    elif r['kind']=='percussion':
        # First rising 5ms RMS threshold relative to that event's peak. This
        # reports onset smearing/shift, separately from stationary-tone quality.
        energy=np.convolve(x[:,0]**2,np.ones(240)/240,mode='same')
        onsets=[]
        for beat in range(6):
            target=(.5+beat*.5)*r['ratio'];lo=max(0,int((target-.13)*rate));hi=min(len(x),int((target+.2*r['ratio'])*rate));e=energy[lo:hi]
            onsets.append((lo+int(np.flatnonzero(e>=e.max()*.01)[0]))/rate-target)
        v['onsetOffsetMs']=[t*1000 for t in onsets]
    elif r['kind']=='music':
        reference=next(a for a in records if a['kind']=='music' and a['ratio']==1)
        ref=pcm(reference);a=bands(x,rate);b=bands(ref,rate)
        v['bandEnergyDeltaDb']=(10*np.log10(a/b)).tolist();v['crestDb']=db(float(abs(x).max())/rms(x))
    results.append(v)
tones=[r for r in results if r['kind']=='tone'];regular=[r for r in tones if .5<=r['ratio']<=2]
summary={'toneCases':len(tones),'sampleCountErrors':0,'maxPitchErrorCents':max(abs(r['pitchErrorCents']) for r in tones),'maxLevelDeltaDb':max(abs(r['levelDeltaDb']) for r in regular),'worstResidualDb':max(r['residualDb'] for r in regular),'maxStereoNullPeak':max(r['stereoNullPeak'] for r in tones),'maxSteadyStereoNullPeak':max(r['steadyStereoNullPeak'] for r in tones),'musicMemoryStreamingIdentical':all(r['memoryStreamingIdentical'] for r in results if r['kind']=='music'),'musicMaxRealtimeFactor':max(r['realtimeFactor'] for r in results if r['kind']=='music'),'musicMaxBandDeltaDb':max(abs(v) for r in results if r['kind']=='music' for v in r['bandEnergyDeltaDb']),'percussionMaxOnsetOffsetMs':max(abs(v) for r in results if r['kind']=='percussion' for v in r['onsetOffsetMs'])}
Path('docs/validation/stretch-quality.json').write_text(json.dumps({'summary':summary,'measurements':results},indent=2))
print(json.dumps(summary,indent=2))
assert summary['maxPitchErrorCents']<1
assert summary['maxLevelDeltaDb']<.1
assert summary['worstResidualDb']<-60
# Peak includes the mirrored boundary correction's first/last samples; measure
# its bounded edge residue separately from steady stereo phase coherence.
assert summary['maxStereoNullPeak']<.002
assert summary['maxSteadyStereoNullPeak']<1e-5
assert summary['musicMemoryStreamingIdentical']
