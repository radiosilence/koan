function qr(id,seed){const g=document.getElementById(id);let s=seed;function r(){s=(s*9301+49297)%233280;return s/233280}
function px(x,y){const R=document.createElementNS('http://www.w3.org/2000/svg','rect');R.setAttribute('x',x);R.setAttribute('y',y);R.setAttribute('width',1);R.setAttribute('height',1);g.appendChild(R)}
function finder(x,y){for(let i=0;i<7;i++)for(let j=0;j<7;j++){if(i==0||j==0||i==6||j==6||(i>1&&i<5&&j>1&&j<5))px(x+i,y+j)}}
finder(0,0);finder(22,0);finder(0,22);for(let x=0;x<29;x++)for(let y=0;y<29;y++){if(!((x<8&&y<8)||(x>20&&y<8)||(x<8&&y>20))&&r()>.52)px(x,y)}}
